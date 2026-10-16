# 开发说明

iPhone Link 是一个 Tauri 2 程序：页面是 `src/` 里的 HTML / JS / CSS，由系统自带的 WebView2 显示；其余都在 `src-tauri/` 的 Rust 里。结构照 [Wakuwaku](https://github.com/zhoupengjie/Wakuwaku) 的写法（一个功能一个 Rust 模块，页面不用框架，`node --test` 和 `cargo test`），目的是以后整块合进 Wakuwaku，见最后一节。

## 原理

```
 Windows                                              iPhone
┌──────────────────────────────┐  USB（go-ios forward）  ┌────────────────────────┐
│ 页面 src/phone/              │                         │ WebDriverAgent (WDA)   │
│   鼠标 → gesture.js → 分数坐标 │ ── 8100：点击/滑动/输入 ─▶│ 苹果的 XCTest 跑着它    │
│   canvas ◀─ Channel 原始 JPEG │ ◀─ 9100：MJPEG 画面流 ── │                        │
│ Rust src-tauri/src/phone.rs  │                         └────────────────────────┘
│   phone/wda.rs   WDA 的 HTTP │
│   phone/mjpeg.rs 切帧        │   go-ios（第一次使用时下载）：列设备、iOS 17+ 隧道、
│   phone/goios.rs 调 go-ios   │   挂开发者镜像、启动 WDA、端口转发、导出照片
└──────────────────────────────┘
```

## 目录

```
src/
  phone/
    index.html      手机窗口的页面
    phone.js        画面（canvas）、鼠标和键盘、输入法、侧边按钮、设置；对手机的操作排队一个一个发（act）
    gesture.js      纯函数：鼠标按下到松开算成点按 / 长按 / 拖动，画面在框里的位置和分数坐标，按键，帧的类型
    phone.css
  shared/i18n.js    中英文，键全在 phone. 下面（和 Wakuwaku 的 i18n.js 同样的形状）
src-tauri/
  src/
    main.rs         独立运行时的外壳：Shared（app、dir、log、phone，名字和 Wakuwaku 的一样）、命令登记、退出时断开
    data.rs         数据目录：IPHONE_LINK_DATA，否则 exe 旁边的 iphone-link-data/
    phone.rs        手机：状态、连接的步骤、页面能调的命令、打开窗口（open）
    phone/
      goios.rs        调 go-ios：每一步的命令、解析它的 JSON、一直跑着的子进程（Kept）、Windows 上随本进程结束的作业
      goios_setup.rs  下载 go-ios：固定版本 v1.3.2，GitHub 给的 SHA-256 校验，解压
      wda.rs          WebDriverAgent 的 HTTP：会话、尺寸、W3C Actions 手势、按键、输入、剪贴板、截图、MJPEG 设置
      mjpeg.rs        MJPEG 切帧；流不可用时退回截图
      zip.rs          解压（Wakuwaku 的 lhm_setup::zip 原样复制，合进去后删掉）
  capabilities/     页面能用的 Tauri 权限：窗口 phone
  icons/
test/
  gesture.test.js   手势、坐标、按键
  i18n.test.js      两种语言键一样；Rust 发出的每个错误码、每个步骤、页面用到的每个词都有文案
  e2e/page.e2e.js   可选：在 Chromium 里跑页面，假的 Tauri，检查页面发出的每个命令（要 playwright-core）
.github/workflows/build.yml   Windows 上测试、编译；打 v* 标签时发布 exe
```

## 构建和运行

```bash
cd src-tauri
cargo run                 # 调试版：有控制台窗口，日志打在控制台
cargo build --release     # target/release/iphone-link.exe：单个文件，没有控制台
cargo test                # Rust 单元测试
cd .. && node --test "test/*.test.js"   # 页面的单元测试（不需要 npm install）
```

页面是打进 exe 的（`tauri.conf.json` 的 `frontendDist`），改了 `src/` 要重新编译。不需要 Node 和 Tauri CLI。

在 Linux 上编译需要 WebKitGTK（`libwebkit2gtk-4.1-dev` 等）；只为类型检查 Windows 专用的代码可以 `rustup target add x86_64-pc-windows-gnu` 装上 MinGW 后 `cargo check --target x86_64-pc-windows-gnu`。

**数据放在哪**（`data.rs`）：`IPHONE_LINK_DATA`；否则 exe 旁边的 `iphone-link-data/`。里面有 go-ios（`goios/v1.3.2/`）、go-ios 的配对记录和开发者镜像（`goios/pair-records`、`goios/devimages`）、WebView2 的目录。放一个 `debug.on` 文件就会写 `debug.log`。

| 环境变量 | 作用 |
| --- | --- |
| `IPHONE_LINK_DATA` | 换一个数据目录 |
| `IPHONE_LINK_GOIOS` | 用这个 `ios` 程序，不下载（测试用假的 go-ios） |
| `IPHONE_LINK_GOIOS_RELEASE` | go-ios 发布信息的 JSON 从这里取（测试用） |
| `IPHONE_LINK_TEST_RELEASES` | `cargo test -- --ignored` 时整个下载流程用的地址（见“测试”） |

## 连接的步骤（phone.rs `connect_usb`）

1. `ios list --details`、`ios devicename`：哪台手机、iOS 几。
2. iOS 17+：`ios tunnel start --userspace`（用户态网络，**不要管理员、不要 wintun.dll**），等它的 agent（127.0.0.1:60105）`/ready`。已经有 go-ios 的 agent 在跑就直接用。
3. `ios forward --port=A:8100 --port=B:9100`：A、B 是本机随便挑的空闲端口，不和别的工具抢 8100。
4. WDA 已经在回答（上次没退出）就到此为止；否则：
5. `ios image auto`：开发者磁盘镜像，第一次下载，失败也继续（已挂载时也会“失败”），WDA 起不来会说。
6. `ios apps --list` 里找 WebDriverAgentRunner（免费 Apple ID 签名后包名常被 Sideloadly 改掉，所以按名字找，`.xctrunner` 结尾的优先）；设置里填了 Bundle ID 就用填的。
7. `ios runwda --bundleid=… --testrunnerbundleid=… --xctestconfig=WebDriverAgentRunner.xctest`，最多等 90 秒；它先退出了就把它最后一行日志作为原因。
8. 会话、屏幕的 point 尺寸、MJPEG 设置（帧率、质量、缩放、`mjpegFixOrientation`）。

一直跑着的 go-ios（隧道、转发、WDA）在 `Conn.kept` 里：断开（`phone::disconnect`）、重连、程序退出（`RunEvent::Exit`）时主动结束；Windows 上它们还都在一个“本进程结束就结束”的作业里（`goios.rs` 的 `job`），崩溃或被任务管理器结束时也不会留下。

## 画面和坐标

- MJPEG 流按 Content-Length 切帧（没有时按 JPEG 的起止标记），原始字节通过 Tauri 的 Channel（`InvokeResponseBody::Raw`）发给页面，页面 `createImageBitmap` 后画进 canvas，画完调 `phone_seen`。没画完的最多两帧，再多就丢帧，不会越积越慢。
- 流连续三次拿不到帧就改为循环截图（慢，但总有画面），每 15 秒试一次能否恢复。
- 页面发来的位置是画面的分数（0~1），Rust 乘上屏幕的 point 尺寸。画面横竖变了，页面调 `phone_size` 重新取尺寸。
- 拖动按鼠标真实的轨迹和速度回放（松手时的速度决定惯性），所以松手后才开始，有零点几秒延迟。

## go-ios 的版本

固定 v1.3.2（`goios_setup.rs` 的 `VERSION`）。`goios.rs` 依赖它的命令、输出和端口：1.3.2 的隧道 agent 在 60105（实测），而它 main 分支的说明里 tunnel info 端口已经写成 28100，`tunnel start` 的参数说明也改过。换版本时要改 `VERSION`、核对 `AGENT_PORT`，在真机上把连接的每一步走一遍。

## 测试

- `node --test "test/*.test.js"`、`cargo test`：不需要手机，也不需要网络。`goios.rs` 的测试用 shell 脚本假装 go-ios（只在 Linux / macOS 上跑），`wda.rs`、`mjpeg.rs` 的测试起一个几十行的假 WDA。
- 整个下载流程：把 go-ios 的 zip 和两份发布 JSON（`release.json` 里是对的 `digest`，`bad.json` 里是错的）放在一个本地 HTTP 服务下，然后
  `IPHONE_LINK_TEST_RELEASES=http://127.0.0.1:8000 cargo test -- --include-ignored`。
- 页面：`npm i --no-save playwright-core && node test/e2e/page.e2e.js`（`SHOTS=<文件夹>` 顺便截图）。
- 不接手机试整个程序：`IPHONE_LINK_GOIOS` 指向一个假的 `ios`（`list`、`devicename`、`apps` 按 go-ios 1.3.2 的格式输出，`forward` 真的把端口转到一个假 WDA），假 WDA 在 8100 回答 HTTP、在 9100 发 MJPEG。

## 合进 Wakuwaku

除了 `zip.rs`，Rust 和页面的文件都可以原样搬过去。它们只用 Wakuwaku 已有的依赖（`ureq 3.4.2`、`base64`、`sha2`、`flate2`、`crc32fast`，版本相同），不引入新的。`phone.rs` 只用到 `crate::{shared, Shared}` 和 `Shared` 的 `app`、`dir`、`log()`、`phone`。

1. **Rust 文件**：`src-tauri/src/phone.rs` 和 `src-tauri/src/phone/`（不要 `zip.rs`）复制到 Wakuwaku 的 `src-tauri/src/`。
2. **解压**：删掉 `phone.rs` 里的 `mod zip;`，`goios_setup.rs` 的 `super::zip::extract` 改成 `crate::lhm_setup::zip::extract`。
3. **代理**：`goios_setup.rs` 的 `agent()` 改成和 `lhm_setup.rs` 一样走 `crate::proxy`（国内连 GitHub 要用）。
4. **main.rs**：
   - 加 `mod phone;`
   - `Shared` 里加 `pub phone: Mutex<phone::Phone>,`，创建处写 `phone: Mutex::default(),`
   - `generate_handler!` 里加 `phone::phone_state` 到 `phone::phone_photos` 这 19 个命令（照本项目 `main.rs` 的列表）
   - 退出时（她的退出流程里）调 `phone::disconnect(&sh)`
5. **打开窗口**：`phone::open` 里的 `WebviewWindowBuilder` 要加上 `.additional_browser_args(crate::browser_args(sh))`，`.data_directory(...)` 改成 `crate::webview_dir(sh)`。**同一个进程里的 WebView2 窗口必须用一样的浏览器参数和数据目录**，不一样新窗口会创建失败。入口挂在托盘菜单、设置或岛上的一个按钮上，调 `phone::open(&sh)`。
6. **权限**：`src-tauri/capabilities/default.json` 的 `windows` 里加 `"phone"`。
7. **页面**：`src/phone/` 复制到 Wakuwaku 的 `src/phone/`；`src/shared/i18n.js` 里 `phone.` 开头的键并进 Wakuwaku 的 `STRINGS`（两边的 `t`、`detectLang` 用法一样）。页面的设置现在存在 `localStorage`（`phone.prefs`），要和 Wakuwaku 的设置放一起时改成走 `settings.rs`。
8. **时间**：`phone.rs` 的 `stamp()` 是 UTC，可以换成 `widgets.rs` 的本地时间。
9. **测试和文档**：`test/gesture.test.js` 搬过去；`test/i18n.test.js` 里检查错误码的部分并进 Wakuwaku 的 `test/i18n.test.js`；`DEVELOPMENT.md` 的目录里加上这几个文件。
10. **数据目录**：go-ios 会放进 `wakuwaku-data/goios/`（`sh.dir`），不用改。

## 已知限制

- 没有声音（WDA 不传音频）。
- 操作有零点几秒延迟；画面帧率取决于 WDA 的 MJPEG 服务，适合日常操作，不适合游戏和视频。
- 免费 Apple ID 签名的 WDA 每 7 天要重新安装一次。
- 多任务按钮模拟的是全面屏的“上滑停顿”，Home 键机型上不对。
