# v1.0.1 GUI 重构建交付

状态：GUI/快捷方式冒烟通过；**未重跑阶段 8，不是已验收候选**。

## 文件

- `Fuji 照片管理器_1.0.1_x64-setup.exe`：推荐安装文件；SHA-256 `d041e1a8f125c0c7b94d7b6f9b8a6c80b597ba68e83a292e16764033640f5981`。
- `local-photo-manager_1.0.1_gui.exe`：构建目录 EXE；SHA-256 `a4c8fde169312c758fe572c47dca03d35228d0baead07ea58fa4bdc40c240642`。
- `local-photo-manager_1.0.1_installed-nsis.exe`：准确安装态 EXE 副本；SHA-256 `56e4605d56fb87139a937b1dedbdd557421385b37328439111fd067c739d4f02`。
- `manifest.json`：机器可读的构建、安装、快捷方式和冒烟绑定。

安装后实际使用的 EXE 位于 `C:\Users\15684\AppData\Local\Fuji 照片管理器\local-photo-manager.exe`。当前桌面和开始菜单快捷方式均已用同一份已验证 `.lnk` 校正；两者 SHA-256 均为 `0dc24edbfaa7fe04e44cbfa21cdcdcd73973127338cb8a3cd3fc0cbecfd8bd83`，启动后的实际进程路径与安装 EXE 一致。

本目录不覆盖 `artifacts/phase-eight-v1.0.1/`。旧 27/27 只绑定旧安装 EXE hash `bc855771910129719c4c56d513c024debd832c23fffe71da909ea2c7079801d8`，不能应用于这里的新二进制。
