# iPhone Link

在 **Windows** 电脑上投屏并**用鼠标键盘操作 iPhone** 的开源工具，用法参考 Windows 自带的“手机连接”（Phone Link）。Rust + Tauri 编写，一个 exe 就能运行，不用装 Python。

> 微软的“手机连接”支持 iPhone 的通知、短信和电话，但**不支持 iPhone 投屏和操作**。这个项目补的就是这一块，两者可以同时使用。
>
> 代码结构按 [Wakuwaku](https://github.com/zhoupengjie/Wakuwaku) 的写法组织，准备以后作为一个功能合进去（见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#合进-wakuwaku)）。

## 功能

| 功能 | 说明 |
| --- | --- |
| 实时投屏 | 通过 USB 显示 iPhone 画面，画质可选“流畅 / 标准 / 高清”，横竖屏自动适配 |
| 鼠标操作 | 单击=点按，按住=长按，拖动=滑动（保留真实速度，可惯性滚动），先按住再拖=拖动图标，滚轮=上下滚动 |
| 键盘输入 | 点一下画面就能直接打字，**支持中文输入法**；退格、回车可用；Ctrl+V 粘贴电脑剪贴板 |
| 系统操作 | 主屏幕 (F1)、多任务 (F2)、通知中心 (F3)、控制中心 (F4)、锁屏/唤醒 (F5)、音量 +/− |
| 截图 | 原分辨率截图，保存到“图片\iPhone Link” (F6) |
| 剪贴板 | 电脑剪贴板 → 手机输入框、电脑 ↔ 手机剪贴板互传 |
| 导出照片 | 把手机相册（DCIM）整体复制到“图片\iPhone Link” |

## 原理

```
 Windows                                          iPhone
┌────────────────────────┐  USB（go-ios 端口转发）  ┌──────────────────────┐
│ iPhone Link            │ ── 点击 / 滑动 / 输入 ──▶ │ WebDriverAgent (WDA) │
│ (Rust + Tauri)         │ ◀──── MJPEG 画面流 ───── │ 苹果官方 XCTest 框架 │
└────────────────────────┘                          └──────────────────────┘
        │ go-ios：识别设备、iOS 17+ 隧道、挂载开发者镜像、启动 WDA
```

- iOS 不允许普通 App 远程控制手机，不越狱的唯一办法是借助苹果的自动化测试框架 XCTest。[WebDriverAgent](https://github.com/appium/WebDriverAgent)（Appium 维护的开源 XCTest 程序）装到手机上后，可以接收点击、滑动、按键指令，并输出画面。
- [go-ios](https://github.com/danielpaulus/go-ios)（MIT）负责在 Windows 上和 iPhone 通信，不需要 Mac 和 Xcode。iOS 17 及以上需要的隧道用它的用户态模式建立，**不需要管理员权限**。go-ios 在第一次使用时由程序从 GitHub 下载，并校验 SHA-256。

## 准备工作（只需做一次）

### 1. 电脑端

1. Windows 10 / 11（需要 WebView2，Windows 11 自带，Windows 10 一般已经装好）。
2. 安装苹果的 USB 驱动：从 Microsoft Store 安装 **Apple Devices**，或者安装 **iTunes**（二选一）。
3. 从 [Releases](https://github.com/ZhihanX/iphone-link/releases) 下载 **`iPhoneLink.exe`**，放在一个单独的文件夹里（它会在旁边建一个 `iphone-link-data` 文件夹存放数据）。

### 2. 在 iPhone 上安装 WebDriverAgent

1. 打开 [WebDriverAgent 的 Releases 页面](https://github.com/appium/WebDriverAgent/releases/latest)，下载 **`WebDriverAgentRunner-Runner.zip`**（真机版本；带 `Sim` 的是模拟器版本，不要下载）。
2. 解压得到 `WebDriverAgentRunner-Runner.app` 文件夹。新建一个名为 **`Payload`** 的文件夹，把 `.app` 放进去，把 `Payload` 文件夹压缩成 zip，再把扩展名改为 **`.ipa`**，例如 `WDA.ipa`。
3. 安装 [Sideloadly](https://sideloadly.io/)，用数据线连接 iPhone，把 `WDA.ipa` 拖进 Sideloadly，登录你的 Apple ID 后点击 **Start**。
   - 可以用免费 Apple ID，但签名只有 **7 天有效期**，过期后要用 Sideloadly 重新安装一次；付费开发者账号有效期一年。
4. 在 iPhone 上：
   - **设置 → 通用 → VPN 与设备管理**：信任你的 Apple ID 对应的开发者证书。
   - **设置 → 隐私与安全性 → 开发者模式**：打开，手机会重启（iOS 16 及以上需要）。

## 使用

1. 双击 `iPhoneLink.exe`。第一次运行时点 **下载 go-ios**（约 9 MB）。
2. 用数据线连接 iPhone，解锁手机；弹出“要信任此电脑吗？”就点 **信任**。
3. 在顶部选择设备，点 **连接**。第一次连接会下载并挂载开发者磁盘镜像，可能要等一两分钟，之后会快很多。

## 常见问题

**找不到设备 / 提示找不到 USB 服务**
确认已安装 Apple Devices 或 iTunes，换一根能传数据的线，手机解锁并点了“信任”，然后点 **刷新**。

**下载 go-ios 失败**
程序需要访问 GitHub。连不上时可以手动下载 [go-ios v1.3.2 的 go-ios-win.zip](https://github.com/danielpaulus/go-ios/releases/tag/v1.3.2)，把里面的 `ios.exe` 放到 `iphone-link-data\goios\v1.3.2\` 里。

**提示“没有找到 WebDriverAgentRunner”**
WDA 还没装好，按上面第 2 步操作。如果用其他方式安装、包名不同，可以在 **设置 → WDA Bundle ID** 里手动填写。

**WebDriverAgent 启动后退出了**
依次检查：开发者模式是否已开启、开发者证书是否已信任、免费签名是否超过了 7 天。点 **日志** 可以看到详细原因。

**画面卡顿**
把画质切到“流畅”。WDA 的画面流帧率有限，适合日常点 App、回消息，不适合玩游戏或看视频。

**操作有延迟**
拖动是在松开鼠标后，按原轨迹在手机上回放一次，所以会有零点几秒的延迟，这是 WDA 的工作方式决定的。

**打字没反应**
需要先在手机上点开一个输入框、弹出键盘，再在电脑上打字。

**多任务按钮没用**
它模拟的是全面屏手机的“上滑并停顿”手势；有 Home 键的老机型请用主屏幕按钮。

**能不能无线连接？**
默认走 USB。高级用户可以在 **设置** 里填写“直连 WDA 地址”（例如 `http://手机IP:8100`），前提是 WDA 已经在手机上运行。

## 已知限制

- 没有声音（WDA 不传输音频）。
- 通知、短信、电话请使用微软的“手机连接”，本项目不重复实现。
- 免费 Apple ID 签名每 7 天要重新安装一次 WDA。

## 从源码编译

```bash
cd src-tauri
cargo build --release      # 生成 target/release/iphone-link.exe
cargo test
cd .. && node --test "test/*.test.js"
```

目录结构、连接流程、测试方法，以及如何合进 Wakuwaku，见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 致谢

- [appium/WebDriverAgent](https://github.com/appium/WebDriverAgent)
- [danielpaulus/go-ios](https://github.com/danielpaulus/go-ios)
- [zhoupengjie/Wakuwaku](https://github.com/zhoupengjie/Wakuwaku)：项目结构，以及 `src-tauri/src/phone/zip.rs`（MIT）

## 许可证

[MIT](LICENSE)
