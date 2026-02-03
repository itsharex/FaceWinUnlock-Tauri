# FaceWinUnlock-Tauri

面容识别解锁核心服务。

## 功能特性

- 支持 Windows 系统的人脸识别身份验证
- 解锁速度快、安全性高
- 跨平台桌面应用程序
- 现代化用户界面

## 🚀 快速开始

### 前置条件

1. **Rust**: 1.90.0 (1159e78c4 2025-09-14) (包含 `cargo` 工具链)
2. **Visual Studio**: 包含 C++ 桌面开发组件 (用于编译 DLL)
3. **OpenCV 环境**: 确保系统已安装 OpenCV 运行时

### 安装与运行

1. **克隆仓库**
```bash
git clone https://github.com/zs1083339604/FaceWinUnlock-Tauri.git
或
git clone git@gitee.com:lieranhuasha/face-win-unlock-tauri.git

cd FaceWinUnlock-Tauri
cd Unlock
```


2. **安装依赖**
```bash
cargo build
```


3. **开发模式运行**
```bash
cargo run
```


4. **构建发行版**
```bash
cargo build --release
```