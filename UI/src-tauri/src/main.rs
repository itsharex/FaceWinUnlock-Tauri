// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{env, fs, path::PathBuf};

use tauri_plugin_log::log::warn;

fn main() {
    // 设置 WebView2 数据文件夹的环境变量
    // 代码来源：[@Xiao-yu233](https://github.com/Xiao-yu233)
    let exe_dir = match env::current_exe() {
        Ok(exe_path) => exe_path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")), // 如果获取失败，使用当前工作目录
        Err(_) => PathBuf::from("."), // 如果获取可执行文件路径失败，使用当前工作目录
    };

    // 构建默认的 cache 文件夹路径（运行目录下的 cache 文件夹）
    let default_cache_dir = exe_dir.join("cache");
    
    // 尝试创建 cache 文件夹
    let webview_data_dir = match fs::create_dir_all(&default_cache_dir) {
        // 创建成功，使用当前目录下的 cache 文件夹
        Ok(_) => default_cache_dir,
        
        // 创建失败，回退到 ProgramData 路径
        Err(e) => {
            warn!("创建默认 cache 目录失败: {}, 将使用 ProgramData 路径", e);
            let app_data = env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".to_string());
            let fallback_dir = format!("{}\\facewinunlock-tauri", app_data);
            
            // 确保回退目录存在
            let _ = fs::create_dir_all(&fallback_dir);
            PathBuf::from(fallback_dir)
        }
    };
    
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", webview_data_dir);
    
    facewinunlock_tauri_lib::run()
}
