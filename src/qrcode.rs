//! QR 码生成器 — 用 embedded-qr (no_std + alloc)
//!
//! 自动选版本 (1~6)，把 QrMatrix 转成 row-major 矩阵供 OLED 渲染。

#![no_std]
extern crate alloc;
use alloc::vec::Vec;

use embedded_qr::{
    QrBuilder, EccLevel, Version,
    Version1, Version2, Version3, Version4, Version5, Version6,
};

/// 扁平 row-major QR 矩阵
pub struct QrCode {
    pub size: u8,
    data: Vec<u8>, // 0=light, 1=dark
}

impl QrCode {
    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.size as usize || y >= self.size as usize {
            return false;
        }
        self.data[y * self.size as usize + x] != 0
    }
}

/// 把任意版本的 QrMatrix 转成 QrCode
fn from_matrix<T: Version>(matrix: &embedded_qr::QrMatrix<T>) -> QrCode {
    let size = matrix.width() as u8;
    let n = size as usize;
    let mut data = alloc::vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            if matrix.get(x, y) {
                data[y * n + x] = 1;
            }
        }
    }
    QrCode { size, data }
}

macro_rules! try_encode {
    ($data:expr, $($v:ty),+) => {
        $(
            if let Ok(matrix) = QrBuilder::<$v>::new()
                .with_ecc_level(EccLevel::M)
                .build($data)
            {
                return Ok(from_matrix(&matrix));
            }
        )+
    };
}

/// 编码字节数据，自动选版本 1~6
pub fn encode(data: &[u8]) -> Result<QrCode, ()> {
    try_encode!(data, Version2, Version3, Version4, Version5, Version6);
    // Version1 兜底
    if let Ok(matrix) = QrBuilder::<Version1>::new()
        .with_ecc_level(EccLevel::M)
        .build(data)
    {
        return Ok(from_matrix(&matrix));
    }
    Err(())
}

/// 生成 WiFi QR 码 (WIFI:S:<ssid>;T:WPA;P:<pass>;;)
pub fn encode_wifi(ssid: &str, password: &str) -> Result<QrCode, ()> {
    let s = alloc::format!("WIFI:S:{};T:WPA;P:{};;", ssid, password);
    encode(s.as_bytes())
}
