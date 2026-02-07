use std::{fs::File, thread as std_thread};

use global::{get_global_log_path, set_global_hwnd};
use log::{info, LevelFilter};
use simplelog::{CombinedLogger, ConfigBuilder, TermLogger, WriteLogger};
use thread::{connect_sqlite, get_install_path, pipe_message_loop};
use windows::{
    core::{w, Error, PCWSTR}, Win32::{
        Foundation::{E_UNEXPECTED, HWND},
        Graphics::Gdi::HBRUSH,
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
            RemoteDesktop::{
                WTSRegisterSessionNotification, NOTIFY_FOR_ALL_SESSIONS
            },
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, CW_USEDEFAULT, HCURSOR, HICON, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WNDCLASSW, WNDCLASS_STYLES, WS_OVERLAPPEDWINDOW
        },
    }
};

use crate::{proc::lock, utils::is_locked};

pub mod global;
pub mod utils;
pub mod proc;
pub mod thread;
pub mod pipe;
pub mod face;

// 注册窗口类并创建窗口
fn create_message_window() -> windows::core::Result<HWND> {
    unsafe {
        let h_instance = GetModuleHandleW(None)?;

        // 定义窗口类
        let class_name = w!("InvisibleMessageWindowClass");

        // 注册窗口类
        let wnd_class = WNDCLASSW {
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(proc::window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance.into(),
            hIcon: HICON::default(),
            hCursor: HCURSOR::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::default(),
            lpszClassName: class_name,
        };

        let atom = RegisterClassW(&wnd_class);
        if atom == 0 {
            return Err(Error::new(E_UNEXPECTED, "注册窗口类失败"));
        }

        // 创建隐藏窗口
        let h_wnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("InvisibleWindow"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE), // 创建消息窗口
            None,
            Some(h_instance.into()),
            None,
        )?;

        if h_wnd.is_invalid() {
            return Err(Error::new(E_UNEXPECTED, "创建隐藏窗口失败"));
        }

        // 注册会话通知
        WTSRegisterSessionNotification(
            h_wnd,
            NOTIFY_FOR_ALL_SESSIONS, // 监听所有会话
        )?;

        Ok(h_wnd)
    }
}

fn main() -> windows::core::Result<()> {
    println!("正在初始化...");
    let pipe_thread = std_thread::spawn(pipe_message_loop);
    println!("获取软件安装目录...");
    let thread = std_thread::spawn(get_install_path);
    thread.join().unwrap();
    println!("初始化日志系统...");
    
    // 初始化日志系统
    if let Ok(file) = File::create(get_global_log_path().join("logs").join("unlock.log")) {
        // 日志时间太麻烦，不搞了，没有日期影响不大
        if let Ok(config) = ConfigBuilder::new().set_time_offset_to_local(){
            match CombinedLogger::init(
                vec![
                    TermLogger::new(
                        LevelFilter::Info,    // 输出级别和文件一致
                        config.clone().build(),   // 共享配置
                        simplelog::TerminalMode::Stdout, // 输出到标准输出（CMD）
                        simplelog::ColorChoice::Auto,    // 自动适配 CMD 颜色
                    ),
                    WriteLogger::new(
                        LevelFilter::Info, 
                        config.build(), 
                        file
                    ),
                ]
            ) {
                Ok(_) => info!("日志系统初始化成功"),
                _ => {},
            }
        }
    }

    let sql_thread = std_thread::spawn(connect_sqlite);
    sql_thread.join().unwrap();
    info!("数据库初始化完成");

    // 初始化Windows COM
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    // 创建消息窗口
    let hwnd = create_message_window()?;
    set_global_hwnd(hwnd);
    info!("消息窗口创建成功");

    // 判断是否是锁屏状态，准备面容解锁
    // 2026-02-03 由抖音 @mingliang71481 提出并配合修复
    if is_locked() {
        info!("系统处于锁屏状态，准备面容解锁");
        lock(hwnd);
    }

    // 消息循环
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, Some(hwnd), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // 清理COM
    unsafe {
        CoUninitialize();
    }

    info!("程序退出，等待线程退出");
    pipe_thread.join().unwrap();

    Ok(())
}