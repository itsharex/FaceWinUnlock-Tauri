# FaceWinUnlock-Tauri

**FaceWinUnlock-Tauri** 是一款基于 Tauri 框架开发的现代化 Windows 面容识别解锁增强软件。它通过自定义 Credential Provider (DLL) 注入 Windows 登录界面，结合前端 Vue 3 和后端 OpenCV 人脸识别算法，为用户提供类似 Windows Hello 的解锁体验。

## ✨ 特性

* **现代化 UI**: 基于 Element Plus 构建。
* **系统级集成**: 自动注册 WinLogon 凭据提供程序 (Credential Provider)。
* **双账户支持**: 同时支持本地账户 (Local Account) 与微软联机账户 (MSA) 解锁。
* **轻量级后端**: Rust 后端确保了高效的文件 IO 处理与注册表操作安全性。
* **隐私保护**: 所有面容特征数据与系统凭据均通过 SQLite 本地存储，不上传云端。

## 🛠️ 技术栈

* **前端界面**: Vue 3 (Composition API), Vue-Router, Pinia, Element Plus
* **后端接口**: Rust (Tauri), Windows API
* **数据库**: SQLite 3
* **面容识别**: OpenCV (人脸检测与特征比对)
* **解锁组件**: 纯Rust 编写的 WinLogon 注入组件

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
cd UI
```


2. **安装依赖**
```bash
npm install
```


3. **开发模式运行**
```bash
npm run tauri dev
```


4. **构建发行版**
```bash
npm run tauri build
```

5. **资源文件**
- [FaceWinUnlock-Tauri.dll](/Server)，编译后得到dll
- [FaceWinUnlock-Server.exe](/Unlock)，编译后得到exe
- [face_detection_yunet_2023mar.onnx](https://github.com/opencv/opencv_zoo/blob/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx)
- [face_recognition_sface_2021dec.onnx](https://github.com/opencv/opencv_zoo/blob/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx)
- [detect.onnx](https://modelscope.cn/models/iic/cv_manual_face-liveness_flrgb/summary)
- [opencv_world4120.dll](https://github.com/opencv/opencv/releases/tag/4.12.0)，需要下载opencv源代码进行编译，[编译教程点这](https://www.cnblogs.com/-CO-/p/18075315)

## 📂 项目结构

```text
├── src/                # Vue 前端源代码
│   ├── components/     # 复用组件 (如账号验证组件)
│   ├── layout/         # 系统主布局
│   ├── views/          # 页面 (初始化、面容管理、设置等)
│   └── utils/          # 数据库连接与工具函数
├── src-tauri/          # Rust 后端源代码
│   └── src/            # Rust 主逻辑 (权限检查、部署、注册表操作)
└── public/             # 公共资源
```

## ⚠️ 免责声明

本项目涉及修改 Windows 系统注册表及 `C:\Windows\System32` 目录。在使用或二次开发时，请务必了解以下风险：

* 错误修改注册表可能导致系统无法正常登录。
* 建议在虚拟机 (VMware/Hyper-V) 环境中进行调试。
* 作者不对因使用本软件导致的任何数据丢失或系统崩溃负责。

## ⚠️ 当前问题记录

- 面容添加页面包含多次重复的特征点提取操作
- 面容添加页面应添加摄像设备选择、人脸阈值等内容
- 当前用户名密码使用明文存储
- 面容添加页面未添加摄像头选项
- 登录日志由Rust写入数据库，改为JS更好