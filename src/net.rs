//! TCP 服务器 (基于 smoltcp)
//! 监听 :4545, 接收字符串后用 Toast 显示在 OLED 上。
//! 同步轮询，不需要 async executor。
//! 使用 Medium::Ip (不需要以太网帧)。

extern crate alloc;
use alloc::vec;
use alloc::boxed::Box;
use alloc::string::String;

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use critical_section::Mutex;

use smoltcp::iface::{Interface as NetIface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer};
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpCidr, HardwareAddress};

// ===== Toast 共享缓冲区 =====

const TOAST_MAX: usize = 64;
static TOAST_BUF: Mutex<UnsafeCell<[u8; TOAST_MAX]>> =
    Mutex::new(UnsafeCell::new([0; TOAST_MAX]));
static TOAST_LEN: AtomicU8 = AtomicU8::new(0);
static TOAST_READY: AtomicBool = AtomicBool::new(false);

pub fn take_toast() -> Option<[u8; TOAST_MAX]> {
    if TOAST_READY.load(Ordering::Acquire) {
        TOAST_READY.store(false, Ordering::Release);
        let len = TOAST_LEN.load(Ordering::Acquire) as usize;
        let mut buf = [0u8; TOAST_MAX];
        critical_section::with(|cs| {
            let cell = TOAST_BUF.borrow(cs);
            unsafe {
                core::ptr::copy_nonoverlapping(cell.get() as *const u8, buf.as_mut_ptr(), len);
            }
        });
        Some(buf)
    } else {
        None
    }
}

fn store_toast(msg: &[u8]) {
    let n = msg.len().min(TOAST_MAX);
    critical_section::with(|cs| {
        let cell = TOAST_BUF.borrow(cs);
        unsafe {
            core::ptr::copy_nonoverlapping(msg.as_ptr(), cell.get() as *mut u8, n);
        }
    });
    TOAST_LEN.store(n as u8, Ordering::Release);
    TOAST_READY.store(true, Ordering::Release);
}

// ===== smoltcp Device 适配器 =====

struct EspRadioDevice(esp_radio::wifi::Interface<'static>);

impl Device for EspRadioDevice {
    type RxToken<'a> = RxWrap where Self: 'a;
    type TxToken<'a> = TxWrap where Self: 'a;

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.0.receive().map(|(rx, tx)| (RxWrap(rx), TxWrap(tx)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        self.0.transmit().map(TxWrap)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ip;
        caps
    }
}

struct RxWrap(esp_radio::wifi::WifiRxToken);
struct TxWrap(esp_radio::wifi::WifiTxToken);

impl RxToken for RxWrap {
    fn consume<R, F>(self, f: F) -> R
    where F: FnOnce(&[u8]) -> R
    {
        self.0.consume_token(|buf: &mut [u8]| f(buf))
    }
}

impl TxToken for TxWrap {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where F: FnOnce(&mut [u8]) -> R
    {
        self.0.consume_token(len, f)
    }
}

// ===== TCP 服务器 =====

const TCP_BUF_SIZE: usize = 4096;
const PORT: u16 = 4545;

pub struct NetServer {
    iface: NetIface,
    device: EspRadioDevice,
    sockets: SocketSet<'static>,
    handle: SocketHandle,
    rx_buf: [u8; 128],
    tick_count: i64,
    listening: bool,
    connected: bool,
}

impl NetServer {
    pub fn new(iface_raw: esp_radio::wifi::Interface<'static>) -> Self {
        let mut device = EspRadioDevice(iface_raw);

        let mut iface = NetIface::new(
            smoltcp::iface::Config::new(HardwareAddress::Ip),
            &mut device,
            Instant::from_millis(0),
        );
        // 配置 IP (0.0.0.0/0 = 监听所有接口, 由上层 WiFi/路由处理)
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0)).unwrap();
        });

        let rx_store: &'static mut [u8] = Box::leak(Box::new([0u8; TCP_BUF_SIZE]));
        let tx_store: &'static mut [u8] = Box::leak(Box::new([0u8; TCP_BUF_SIZE]));
        let rx_buf = SocketBuffer::new(rx_store);
        let tx_buf = SocketBuffer::new(tx_store);
        let socket = TcpSocket::new(rx_buf, tx_buf);

        let mut sockets = SocketSet::new(vec![]);
        let handle = sockets.add(socket);

        Self {
            iface,
            device,
            sockets,
            handle,
            rx_buf: [0u8; 128],
            tick_count: 0,
            listening: false,
            connected: false,
        }
    }

    /// 返回当前 IP 地址字符串（smoltcp 接口配置）
    pub fn ip_address(&self) -> &str {
        for cidr in self.iface.ip_addrs() {
            let addr = cidr.address();
            if !addr.is_unspecified() {
                let s = alloc::format!("{}", addr);
                return Box::leak(s.into_boxed_str());
            }
        }
        "0.0.0.0"
    }

    pub fn tick(&mut self) {
        self.tick_count += 1;
        let timestamp = Instant::from_millis(self.tick_count * 20);

        // 驱动网络栈
        self.iface.poll(timestamp, &mut self.device, &mut self.sockets);

        let socket: &mut TcpSocket<'static> = self.sockets.get_mut(self.handle);

        // ---- 开始监听（仅一次）----
        if !self.listening && !socket.is_open() {
            if socket.listen((IpAddress::v4(0, 0, 0, 0), PORT)).is_ok() {
                self.listening = true;
                esp_println::println!("[net] Listening on :{}", PORT);
            }
        }

        // ---- 接收数据 ----
        if socket.can_recv() {
            match socket.recv_slice(&mut self.rx_buf) {
                Ok(n) if n > 0 => {
                    self.connected = true;
                    let msg = &self.rx_buf[..n];
                    esp_println::println!("[net] Received {} bytes", n);
                    let mut clean = [0u8; 64];
                    let mut ci = 0;
                    for &b in msg {
                        if ci >= 60 { break; }
                        if b.is_ascii_graphic() || b == b' ' {
                            clean[ci] = b;
                            ci += 1;
                        }
                    }
                    if ci > 0 {
                        store_toast(&clean[..ci]);
                    }
                    let _ = socket.send_slice(msg);
                }
                _ => {}
            }
        }

        // ---- 检测客户端断开（仅在有数据后）----
        if self.connected && socket.is_open() && !socket.may_recv() && !socket.may_send() {
            esp_println::println!("[net] Client disconnected");
            self.connected = false;
            self.listening = false;
            socket.close();
        }
    }
}
