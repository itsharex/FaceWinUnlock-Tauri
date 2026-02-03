use std::{ffi::OsStr, fs, os::windows::ffi::OsStrExt, sync::atomic::Ordering, time::{SystemTime, UNIX_EPOCH}};

use log::info;
use opencv::{core::{Mat, Vector}, imgcodecs::imencode};
use windows::{
    Win32::{
        Foundation::E_UNEXPECTED,
        System::Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
            RegQueryValueExW,
        },
    },
    core::{Error, PCWSTR},
};

use crate::global::RETRY_DELAY;

// 记录上一次发送管道消息的时间戳（毫秒）
static mut LAST_SEND_TIME: u128 = 0;

fn get_system_time() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn set_last_send_time() {
    let time = get_system_time();
    unsafe {
        LAST_SEND_TIME = time;
    }
}

pub fn can_retry() -> bool {
    unsafe {
        // 获取当前时间戳（毫秒）
        let now = get_system_time();

        let delay: u128 = match RETRY_DELAY.load(Ordering::SeqCst).try_into() {
            Ok(interval) => {
                interval
            },
            Err(_) => {
                10000
            }
        };

        // 如果距离上次发送超过最小间隔，更新时间并允许发送
        if now - LAST_SEND_TIME >= delay {
            LAST_SEND_TIME = now;
            true
        } else {
            false
        }
    }
}

/// 读取注册表数据
pub fn read_facewinunlock_registry(key_name: &str) -> windows::core::Result<String> {
    let reg_path = "SOFTWARE\\facewinunlock-tauri";
    // 打开HKLM下的注册表项
    let mut hkey: HKEY = HKEY::default();

    let os_str = OsStr::new(reg_path);
    let reg_path_ptr: Vec<u16> = os_str.encode_wide().chain(std::iter::once(0)).collect();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR::from_raw(reg_path_ptr.as_ptr()), // 子路径
            None,                                    // 保留参数
            KEY_READ,                                // 只读
            &mut hkey,                               // 输出打开的注册表句柄
        )
    };

    if status.is_err() {
        return Err(Error::new(
            E_UNEXPECTED,
            format!("打开注册表失败: {}", status.0),
        ));
    }

    // 查询值的长度
    let mut value_type = REG_VALUE_TYPE::default();
    let mut value_len = 0u32;

    let os_str = OsStr::new(key_name);
    let key_name_ptr: Vec<u16> = os_str.encode_wide().chain(std::iter::once(0)).collect();
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name_ptr.as_ptr()),
            None,
            Some(&mut value_type),
            None,
            Some(&mut value_len),
        )
    };

    if status.is_err() {
        // 关闭注册表
        unsafe {
            let _ = RegCloseKey(hkey);
        };
        return Err(Error::new(
            E_UNEXPECTED,
            format!("查询注册表长度失败: {}", status.0),
        ));
    }

    if value_type != REG_SZ {
        // 关闭注册表
        unsafe {
            let _ = RegCloseKey(hkey);
        };
        return Err(Error::new(E_UNEXPECTED, "值类型不是 REG_SZ"));
    }

    // 读取值内容
    let mut buffer = vec![0u16; (value_len / 2) as usize];
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name_ptr.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr() as *mut u8), // 转换为 *mut u8
            Some(&mut value_len),
        )
    };

    if status.is_err() {
        // 关闭注册表
        unsafe {
            let _ = RegCloseKey(hkey);
        };
        return Err(Error::new(
            E_UNEXPECTED,
            format!("读取注册表值失败: {}", status.0),
        ));
    }

    unsafe {
        let _ = RegCloseKey(hkey);
    };

    // 将 UTF-16 数组转换回 Rust String
    let value = String::from_utf16(&buffer)?
        .trim_end_matches('\0')
        .to_string();
    Ok(value)
}

/// 将 OpenCV Mat 保存为 .faceimg 后缀的 JPG 文件
/// 参数:
/// - mat: 待保存的图像矩阵
/// - path: 保存路径（如 "output.faceimg"）
pub fn save_mat_as_faceimg(mat: &Mat, path: &str) -> Result<(), String> {
    let mut buf = Vector::<u8>::new();
    imencode(".jpg", &mat, &mut buf, &Vector::new()).unwrap();
    fs::write(path, buf).map_err(|e| {
        format!("图片保存失败: {}", e)
    })?;

    info!("图片保存成功");
    Ok(())
}