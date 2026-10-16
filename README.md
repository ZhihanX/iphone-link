# iPhone Link

在 **Windows** 电脑上投屏并**用鼠标键盘操作 iPhone** 的开源工具，界面和用法参考 Windows 自带的“手机连接”（Phone Link）。

> 微软的“手机连接”支持 iPhone 的通知、短信和电话，但**不支持 iPhone 投屏和操作**。这个项目补上的就是这一块，两者可以同时使用。

## 功能

| 功能 | 说明 |
| --- | --- |
| 实时投屏 | 通过 USB 显示 iPhone 画面，画质可选“流畅 / 标准 / 高清” |
| 鼠标操作 | 单击=点按，按住=长按，拖动=滑动（保留真实速度，可惯性滚动），滚轮=上下滚动 |
| 键盘输入 | 点一下画面后直接打字，**支持中文输入法**；退格、回车可用 |
| 系统操作 | 主屏幕 (F1)、多任务 (F2)、通知中心 (F3)、控制中心 (F4)、锁屏/唤醒 (F5)、音量 +/− |
| 截图 | 原分辨率截图，保存到“图片\iPhone Link” (F6) |
| 剪贴板 | 电脑剪贴板 → 手机输入框 (Ctrl+V)、电脑 ↔ 手机剪贴板互传 |
| 导出照片 | 把手机相册（DCIM）整体复制到电脑 |

## 原理

```
 Windows                                          iPhone
┌───────────────────────┐   USB（usbmux 端口转发）  ┌──────────────────────┐
│ iPhone Link (PySide6) │ ── 8100：点击/滑动/输入 ──▶│ WebDriverAgent (WDA) │
│                       │ ◀─ 9100：MJPEG 画面流 ──── │ 苹果官方 XCTest 框架 │
└───────────────────────┘                          └──────────────────────┘
          │ pymobiledevice3：识别设备、挂载开发者镜像、启动 WDA
```

- iOS 不允许普通 App 远程控制手机，唯一不越狱的办法是借助苹果的自动化测试框架 XCTest。[WebDriverAgent](https://github.com/appium/WebDriverAgent) 是 Appium 维护的开源 XCTest 程序，装到手机上后可以接收点击、滑动、按键指令，并输出画面。
- [pymobiledevice3](https://github.com/doronz88/pymobiledevice3) 负责在 Windows 上和 iPhone 通信，不需要 Mac 和 Xcode；iOS 17 及以上需要的隧道也由它自动建立，**无需管理员权限**。

## 准备工作（只需做一次）

### 1. 电脑端

1. Windows 10 / 11。
2. 安装 [Python 3.10 或更高版本](https://www.python.org/downloads/)，安装时勾选 **Add python.exe to PATH**。
3. 安装苹果的 USB 驱动：从 Microsoft Store 安装 **Apple Devices**，或者安装 **iTunes**（二选一）。
4. 下载本项目：点击 GitHub 页面上的 **Code → Download ZIP** 并解压，或者 `git clone`。

### 2. 在 iPhone 上安装 WebDriverAgent

1. 打开 [WebDriverAgent 的 Releases 页面](https://github.com/appium/WebDriverAgent/releases/latest)，下载 **`WebDriverAgentRunner-Runner.zip`**（这是真机版本；带 `Sim` 的是模拟器版本，不要下载）。
2. 解压后得到 `WebDriverAgentRunner-Runner.app` 文件夹。新建一个名为 **`Payload`** 的文件夹，把 `.app` 放进去，然后把 `Payload` 文件夹压缩成 zip，并把扩展名改为 **`.ipa`**，例如 `WDA.ipa`。
3. 安装 [Sideloadly](https://sideloadly.io/)，用数据线连接 iPhone，把 `WDA.ipa` 拖进 Sideloadly，登录你的 Apple ID 后点击 **Start**。
   - 可以用免费 Apple ID，但签名只有 **7 天有效期**，过期后需要重新用 Sideloadly 安装一次；付费开发者账号有效期一年。
4. 在 iPhone 上：
   - **设置 → 通用 → VPN 与设备管理**：信任你的 Apple ID 对应的开发者证书。
   - **设置 → 隐私与安全性 → 开发者模式**：打开，手机会重启（iOS 16 及以上需要）。

## 运行

双击 **`run.bat`**。第一次运行会自动创建虚拟环境并安装依赖，需要几分钟。

也可以手动运行：

```bat
python -m venv .venv
.venv\Scripts\pip install -r requirements.txt
.venv\Scripts\python -m iphone_link
```

然后：

1. 用数据线连接 iPhone，解锁手机，如果弹出“要信任此电脑吗？”就点 **信任**。
2. 在程序顶部选择设备，点击 **连接**。
3. 第一次连接会下载并挂载开发者磁盘镜像，可能要等一两分钟；之后会快很多。

## 常见问题

**找不到设备 / 查找设备失败**
确认已安装 Apple Devices 或 iTunes，换一根能传数据的线，手机解锁并点了“信任”。

**提示“没有找到 WebDriverAgentRunner”**
WDA 还没装好，按上面第 2 步操作。如果你用其他方式安装、包名不同，可以在 **设置 → WDA Bundle ID** 里手动填写。

**WDA 启动失败**
依次检查：开发者模式是否已开启、开发者证书是否已信任、免费签名是否超过了 7 天。点 **显示 / 隐藏日志** 可以看到详细原因。

**画面卡顿**
把画质切到“流畅”。WDA 的画面流本身帧率有限，这个方案适合日常点 App、回消息，不适合玩游戏或看视频。

**操作有延迟**
鼠标拖动是在你松开鼠标后，按原轨迹在手机上回放一次，所以会有零点几秒的延迟，这是 WDA 的工作方式决定的。

**打字没反应**
需要手机上先点开一个输入框、弹出键盘，再在电脑上打字。

**多任务按钮没用**
它模拟的是全面屏手机的“上滑并停顿”手势；有 Home 键的老机型请双击 Home 键（手机上）或直接用主屏幕按钮。

**能不能无线连接？**
默认走 USB。高级用户可以在 **设置** 里填写“直连 WDA 地址”（例如 `http://手机IP:8100`），前提是 WDA 已经在手机上运行。

## 已知限制

- 没有声音（WDA 不传输音频）。
- 通知、短信、电话请使用微软的“手机连接”，本项目不重复实现。
- 免费 Apple ID 签名每 7 天需要重新安装一次 WDA。

## 项目结构

```
iphone_link/
├── app.py          主窗口：按钮、设置、连接流程
├── screen_view.py  画面控件：把鼠标/键盘操作转成手势
├── stream.py       MJPEG 画面流解析
├── wda.py          WebDriverAgent HTTP 客户端
└── device.py       调用 pymobiledevice3：设备列表、启动 WDA、端口转发、导出照片
```

## 致谢

- [appium/WebDriverAgent](https://github.com/appium/WebDriverAgent)
- [doronz88/pymobiledevice3](https://github.com/doronz88/pymobiledevice3)

## 许可证

[MIT](LICENSE)
