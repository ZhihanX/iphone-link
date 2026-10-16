"""iPhone Link 主窗口。"""

from __future__ import annotations

import os
import queue
import sys
import threading
import time
import traceback
from collections.abc import Callable

from PySide6.QtCore import QObject, QSettings, QStandardPaths, Qt, QThread, QTimer, QUrl, Signal
from PySide6.QtGui import QAction, QDesktopServices, QGuiApplication, QIcon, QKeySequence
from PySide6.QtWidgets import (
    QApplication, QComboBox, QDialog, QDialogButtonBox, QDockWidget, QFileDialog, QFormLayout,
    QFrame, QGroupBox, QHBoxLayout, QLabel, QLineEdit, QMainWindow, QMessageBox, QPlainTextEdit,
    QPushButton, QSpinBox, QToolBar, QVBoxLayout, QWidget,
)

from . import __version__
from .device import DeviceInfo, DeviceSession, export_photos, list_devices
from .screen_view import ScreenView
from .stream import FrameReader
from .wda import Size, WDAClient, WDAError

QUALITY_PRESETS = {
    "流畅": dict(fps=30, quality=40, scale=40),
    "标准": dict(fps=30, quality=60, scale=50),
    "高清": dict(fps=24, quality=85, scale=75),
}


# ---------------------------------------------------------------------- 线程工具

class Task(QThread):
    """在后台线程运行一个函数，结束后在主线程回调。"""

    done = Signal(object)
    failed = Signal(str)

    def __init__(self, fn: Callable, parent=None):
        super().__init__(parent)
        self.fn = fn

    def run(self) -> None:
        try:
            self.done.emit(self.fn())
        except Exception as e:  # noqa: BLE001 - 错误统一显示给用户
            self.failed.emit(str(e) or traceback.format_exc())


class ActionWorker(QObject):
    """串行执行对手机的操作，保证点击、输入的先后顺序，且不卡界面。"""

    error = Signal(str)
    info = Signal(str)

    def __init__(self):
        super().__init__()
        self._q: queue.Queue = queue.Queue()
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()

    def submit(self, fn: Callable, ok_message: str | None = None) -> None:
        self._q.put((fn, ok_message))

    def clear(self) -> None:
        try:
            while True:
                self._q.get_nowait()
        except queue.Empty:
            pass

    def _loop(self) -> None:
        while True:
            fn, ok_message = self._q.get()
            try:
                fn()
                if ok_message:
                    self.info.emit(ok_message)
            except WDAError as e:
                self.error.emit(str(e))
            except Exception as e:  # noqa: BLE001
                self.error.emit(f"{type(e).__name__}: {e}")


class LogBridge(QObject):
    line = Signal(str)
    status = Signal(str)


# ---------------------------------------------------------------------- 设置

class SettingsDialog(QDialog):
    def __init__(self, settings: QSettings, parent=None):
        super().__init__(parent)
        self.setWindowTitle("设置")
        self.settings = settings
        form = QFormLayout(self)

        self.bundle = QLineEdit(settings.value("wda_bundle", "", str))
        self.bundle.setPlaceholderText("留空则自动查找 WebDriverAgentRunner")
        form.addRow("WDA Bundle ID", self.bundle)

        self.wda_port = QSpinBox()
        self.wda_port.setRange(1024, 65535)
        self.wda_port.setValue(int(settings.value("wda_port", 8100)))
        form.addRow("本地 WDA 端口", self.wda_port)

        self.mjpeg_port = QSpinBox()
        self.mjpeg_port.setRange(1024, 65535)
        self.mjpeg_port.setValue(int(settings.value("mjpeg_port", 9100)))
        form.addRow("本地画面端口", self.mjpeg_port)

        hint = QLabel("高级：如果 WDA 已经由别的工具启动（或想通过 Wi-Fi 直连手机 IP），"
                      "填写下面两项后点“连接”会跳过 USB 启动步骤。")
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray;")
        form.addRow(hint)
        self.direct_wda = QLineEdit(settings.value("direct_wda", "", str))
        self.direct_wda.setPlaceholderText("例如 http://192.168.1.23:8100")
        form.addRow("直连 WDA 地址", self.direct_wda)
        self.direct_mjpeg = QLineEdit(settings.value("direct_mjpeg", "", str))
        self.direct_mjpeg.setPlaceholderText("例如 http://192.168.1.23:9100")
        form.addRow("直连画面地址", self.direct_mjpeg)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok
                                   | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self._save)
        buttons.rejected.connect(self.reject)
        form.addRow(buttons)

    def _save(self) -> None:
        s = self.settings
        s.setValue("wda_bundle", self.bundle.text().strip())
        s.setValue("wda_port", self.wda_port.value())
        s.setValue("mjpeg_port", self.mjpeg_port.value())
        s.setValue("direct_wda", self.direct_wda.text().strip())
        s.setValue("direct_mjpeg", self.direct_mjpeg.text().strip())
        self.accept()


# ---------------------------------------------------------------------- 主窗口

class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle(f"iPhone Link {__version__}")
        self.settings = QSettings("iPhoneLink", "iPhoneLink")

        self.devices: list[DeviceInfo] = []
        self.session: DeviceSession | None = None
        self.wda: WDAClient | None = None
        self.reader: FrameReader | None = None
        self.points = Size(0, 0)            # 手机屏幕的逻辑尺寸（point）
        self._last_aspect: bool | None = None
        self._resized_once = False
        self._tasks: set[Task] = set()
        self._pending_text = ""
        self._wheel_acc = 0
        self._wheel_pos = (0.5, 0.5)

        self.log_bridge = LogBridge()
        self.worker = ActionWorker()
        self.worker.error.connect(lambda m: self._status(f"操作失败：{m}", error=True))
        self.worker.info.connect(self._status)

        self._build_ui()
        self.log_bridge.line.connect(self._append_log)
        self.log_bridge.status.connect(lambda m: (self.statusBar().showMessage(m),
                                                  self.view.set_placeholder(m)))

        self._text_timer = QTimer(self, singleShot=True, interval=80)
        self._text_timer.timeout.connect(self._flush_text)
        self._wheel_timer = QTimer(self, singleShot=True, interval=120)
        self._wheel_timer.timeout.connect(self._flush_wheel)

        self._set_connected(False)
        QTimer.singleShot(0, self.refresh_devices)

    # ------------------------------------------------------------------ 界面

    def _build_ui(self) -> None:
        tb = QToolBar("设备")
        tb.setMovable(False)
        self.addToolBar(tb)
        tb.addWidget(QLabel(" 设备："))
        self.device_combo = QComboBox()
        self.device_combo.setMinimumWidth(280)
        tb.addWidget(self.device_combo)
        self.refresh_act = QAction("刷新", self, triggered=self.refresh_devices)
        tb.addAction(self.refresh_act)
        tb.addSeparator()
        self.connect_btn = QPushButton("连接")
        self.connect_btn.clicked.connect(self.toggle_connection)
        tb.addWidget(self.connect_btn)
        tb.addSeparator()
        tb.addAction(QAction("设置", self, triggered=self.open_settings))
        tb.addAction(QAction("帮助", self, triggered=lambda: QDesktopServices.openUrl(
            QUrl("https://github.com/ZhihanX/iphone-link#readme"))))

        central = QWidget()
        layout = QHBoxLayout(central)
        layout.setContentsMargins(6, 6, 6, 6)

        self.view = ScreenView()
        self.view.set_placeholder("选择设备后点击“连接”")
        self.view.tapped.connect(self.on_tap)
        self.view.long_pressed.connect(self.on_long_press)
        self.view.dragged.connect(self.on_drag)
        self.view.scrolled.connect(self.on_scroll)
        self.view.text_entered.connect(self.queue_text)
        self.view.paste_requested.connect(self.paste_pc_clipboard)
        layout.addWidget(self.view, 1)

        layout.addWidget(self._build_side_panel())
        self.setCentralWidget(central)

        self.log_view = QPlainTextEdit(readOnly=True)
        self.log_view.setMaximumBlockCount(3000)
        self.log_dock = QDockWidget("日志", self)
        self.log_dock.setWidget(self.log_view)
        self.addDockWidget(Qt.DockWidgetArea.BottomDockWidgetArea, self.log_dock)
        self.log_dock.hide()

        self.statusBar().showMessage("就绪")
        self.resize(760, 900)

    def _build_side_panel(self) -> QWidget:
        panel = QWidget()
        panel.setFixedWidth(220)
        v = QVBoxLayout(panel)
        v.setContentsMargins(0, 0, 0, 0)

        self.info_label = QLabel("未连接")
        self.info_label.setWordWrap(True)
        self.info_label.setFrameShape(QFrame.Shape.StyledPanel)
        self.info_label.setMargin(6)
        v.addWidget(self.info_label)

        self._controls: list[QWidget] = []

        def button(text: str, slot, shortcut: str | None = None, tip: str | None = None) -> QPushButton:
            b = QPushButton(text)
            b.clicked.connect(slot)
            if shortcut:
                b.setShortcut(QKeySequence(shortcut))
                b.setToolTip(f"{tip or text}（{shortcut}）")
            elif tip:
                b.setToolTip(tip)
            self._controls.append(b)
            return b

        nav = QGroupBox("导航")
        g = QVBoxLayout(nav)
        g.addWidget(button("主屏幕", self.go_home, "F1"))
        g.addWidget(button("多任务", self.app_switcher, "F2", "上滑并停顿，打开后台应用列表"))
        g.addWidget(button("通知中心", self.notification_center, "F3"))
        g.addWidget(button("控制中心", self.control_center, "F4"))
        v.addWidget(nav)

        hw = QGroupBox("按键")
        g = QVBoxLayout(hw)
        row = QHBoxLayout()
        row.addWidget(button("音量 +", lambda: self._run(lambda w: w.press_button("volumeUp"))))
        row.addWidget(button("音量 −", lambda: self._run(lambda w: w.press_button("volumeDown"))))
        g.addLayout(row)
        g.addWidget(button("锁屏 / 唤醒", self.toggle_lock, "F5"))
        g.addWidget(button("截图保存", self.save_screenshot, "F6"))
        v.addWidget(hw)

        txt = QGroupBox("文字与剪贴板")
        g = QVBoxLayout(txt)
        self.text_input = QLineEdit()
        self.text_input.setPlaceholderText("输入文字，回车发送到手机")
        self.text_input.returnPressed.connect(self.send_text_input)
        self._controls.append(self.text_input)
        g.addWidget(self.text_input)
        g.addWidget(button("电脑剪贴板 → 手机输入", self.paste_pc_clipboard,
                           tip="把电脑剪贴板里的文字输入到手机当前输入框（也可在画面上按 Ctrl+V）"))
        g.addWidget(button("电脑剪贴板 → 手机剪贴板", self.push_clipboard))
        g.addWidget(button("手机剪贴板 → 电脑", self.pull_clipboard))
        v.addWidget(txt)

        other = QGroupBox("其他")
        g = QVBoxLayout(other)
        g.addWidget(button("导出相册照片…", self.export_photos))
        row = QHBoxLayout()
        row.addWidget(QLabel("画质"))
        self.quality_combo = QComboBox()
        self.quality_combo.addItems(list(QUALITY_PRESETS))
        self.quality_combo.setCurrentText(self.settings.value("quality", "标准", str))
        self.quality_combo.currentTextChanged.connect(self.apply_quality)
        self._controls.append(self.quality_combo)
        row.addWidget(self.quality_combo, 1)
        g.addLayout(row)
        log_btn = QPushButton("显示 / 隐藏日志")
        log_btn.clicked.connect(lambda: self.log_dock.setVisible(not self.log_dock.isVisible()))
        g.addWidget(log_btn)
        v.addWidget(other)

        tips = QLabel("单击=点按　按住=长按\n拖动=滑动　滚轮=滚动\n点一下画面后可直接打字")
        tips.setStyleSheet("color: gray;")
        v.addWidget(tips)
        v.addStretch(1)
        return panel

    def _set_connected(self, connected: bool, busy: bool = False) -> None:
        for w in self._controls:
            w.setEnabled(connected)
        self.connect_btn.setText("断开" if connected else ("连接中…" if busy else "连接"))
        self.connect_btn.setEnabled(not busy)
        self.device_combo.setEnabled(not connected and not busy)
        self.refresh_act.setEnabled(not connected and not busy)

    def _status(self, msg: str, error: bool = False) -> None:
        self.statusBar().showMessage(msg, 8000 if error else 5000)
        self.log(("[错误] " if error else "") + msg)

    def log(self, line: str) -> None:
        # 可在任意线程调用
        self.log_bridge.line.emit(f"{time.strftime('%H:%M:%S')} {line}")

    def _append_log(self, line: str) -> None:
        self.log_view.appendPlainText(line)

    def _start_task(self, fn: Callable, on_done: Callable | None = None,
                    on_fail: Callable | None = None) -> None:
        task = Task(fn, self)
        self._tasks.add(task)
        if on_done:
            task.done.connect(on_done)
        task.failed.connect(on_fail or (lambda m: self._status(m, error=True)))
        task.finished.connect(lambda: self._tasks.discard(task))
        task.start()

    # ------------------------------------------------------------------ 设备与连接

    def refresh_devices(self) -> None:
        self.device_combo.clear()
        self.device_combo.addItem("正在查找设备…")
        self.refresh_act.setEnabled(False)

        def done(devices: list[DeviceInfo]) -> None:
            self.devices = devices
            self.device_combo.clear()
            for d in devices:
                self.device_combo.addItem(d.label())
            if not devices:
                self.device_combo.addItem("未发现设备（请用数据线连接并信任此电脑）")
            self.refresh_act.setEnabled(True)
            self._status(f"发现 {len(devices)} 台设备")

        def fail(msg: str) -> None:
            self.device_combo.clear()
            self.device_combo.addItem("查找设备失败（详见日志）")
            self.refresh_act.setEnabled(True)
            self._status("查找设备失败。Windows 需要安装 iTunes 或 Apple Devices 以提供 USB 驱动。",
                         error=True)
            self.log(msg)

        self._start_task(list_devices, done, fail)

    def toggle_connection(self) -> None:
        if self.wda is not None:
            self.disconnect_device()
        else:
            self.connect_device()

    def connect_device(self) -> None:
        direct_wda = self.settings.value("direct_wda", "", str).strip()
        direct_mjpeg = self.settings.value("direct_mjpeg", "", str).strip()
        device: DeviceInfo | None = None
        if not direct_wda:
            idx = self.device_combo.currentIndex()
            if idx < 0 or idx >= len(self.devices):
                QMessageBox.information(self, "提示", "请先用数据线连接 iPhone，并点击“刷新”。")
                return
            device = self.devices[idx]

        self._set_connected(False, busy=True)
        self.view.set_placeholder("正在连接…")

        def progress(msg: str) -> None:
            self.log(msg)
            self.log_bridge.status.emit(msg)

        def work():
            if direct_wda:
                wda = WDAClient(direct_wda)
                self.log(f"直连 WDA：{direct_wda}")
                if not wda.is_ready():
                    raise RuntimeError(f"无法连接 {direct_wda}")
                session, mjpeg_url = None, direct_mjpeg or direct_wda.rsplit(":", 1)[0] + ":9100"
            else:
                session = DeviceSession(
                    device, self.log,
                    wda_port=int(self.settings.value("wda_port", 8100)),
                    mjpeg_port=int(self.settings.value("mjpeg_port", 9100)),
                    bundle_id=self.settings.value("wda_bundle", "", str).strip() or None,
                )
                try:
                    wda = session.start(progress)
                except Exception:
                    session.stop()
                    raise
                mjpeg_url = session.mjpeg_url
            try:
                wda.session_id()
                size = wda.window_size()
                preset = QUALITY_PRESETS.get(self.quality_combo.currentText(), QUALITY_PRESETS["标准"])
                try:
                    wda.configure_mjpeg(**preset)
                    wda.update_settings(mjpegFixOrientation=True)
                except WDAError as e:
                    self.log(f"设置画面参数失败（不影响使用）：{e}")
            except Exception:
                if session:
                    session.stop()
                raise
            return session, wda, mjpeg_url, size

        self._start_task(work, lambda r: self._on_connected(device, *r), self._on_connect_failed)

    def _on_connected(self, device, session, wda, mjpeg_url, size) -> None:
        self.session, self.wda, self.points = session, wda, size
        self._last_aspect = None
        self._resized_once = False
        name = device.label() if device else wda.base_url
        self.info_label.setText(f"已连接：{name}\n屏幕：{size.width:.0f}×{size.height:.0f} pt")
        self.reader = FrameReader(mjpeg_url, wda, self)
        self.reader.frame.connect(self._on_frame)
        self.reader.status.connect(self._status)
        self.reader.start()
        self._set_connected(True)
        self.view.set_placeholder("等待画面…")
        self.view.setFocus()
        self._status("连接成功")

    def _on_connect_failed(self, msg: str) -> None:
        self._set_connected(False)
        self.view.set_placeholder("连接失败")
        self.log(msg)
        self.log_dock.show()
        QMessageBox.warning(self, "连接失败", msg)

    def disconnect_device(self) -> None:
        self.worker.clear()
        if self.reader:
            self.reader.stop()
            self.reader = None
        if self.session:
            self.session.stop()
            self.session = None
        self.wda = None
        self.info_label.setText("未连接")
        self.view.set_placeholder("已断开")
        self._set_connected(False)
        self._status("已断开")

    def open_settings(self) -> None:
        SettingsDialog(self.settings, self).exec()

    def apply_quality(self, name: str) -> None:
        self.settings.setValue("quality", name)
        preset = QUALITY_PRESETS[name]
        self._run(lambda w: w.configure_mjpeg(**preset), f"画质已切换为“{name}”")

    # ------------------------------------------------------------------ 画面

    def _on_frame(self, img) -> None:
        self.view.set_image(img)
        landscape = img.width() > img.height()
        if landscape != self._last_aspect:
            # 横竖屏切换后重新获取屏幕尺寸，保证点击坐标准确
            if self._last_aspect is not None:
                self._refresh_size()
            self._last_aspect = landscape
        if not self._resized_once:
            self._resized_once = True
            self._fit_window(img.width(), img.height())

    def _refresh_size(self) -> None:
        def work():
            wda = self.wda
            if wda is not None:
                size = wda.window_size()
                self.points = size
        self.worker.submit(work)

    def _fit_window(self, w: int, h: int) -> None:
        avail = self.view.screen().availableGeometry() if self.view.screen() else None
        if avail is None:
            return
        target_h = int(avail.height() * 0.8)
        target_w = int(target_h * w / h)
        extra_w = self.width() - self.view.width()
        extra_h = self.height() - self.view.height()
        self.resize(min(target_w + extra_w, avail.width()), min(target_h + extra_h, avail.height()))

    # ------------------------------------------------------------------ 手势

    def _pt(self, nx: float, ny: float) -> tuple[float, float]:
        return nx * self.points.width, ny * self.points.height

    def _run(self, action: Callable[[WDAClient], None], ok_message: str | None = None) -> None:
        wda = self.wda
        if wda is None:
            return
        self.worker.submit(lambda: action(wda), ok_message)

    def on_tap(self, nx: float, ny: float) -> None:
        x, y = self._pt(nx, ny)
        self._run(lambda w: w.tap(x, y))

    def on_long_press(self, nx: float, ny: float, ms: int) -> None:
        x, y = self._pt(nx, ny)
        self._run(lambda w: w.long_press(x, y, min(ms, 3000)))

    def on_drag(self, path: list, hold_ms: int) -> None:
        pts = [(*self._pt(nx, ny), dt) for nx, ny, dt in path]
        self._run(lambda w: w.drag_path(pts, hold_start_ms=min(hold_ms, 2000)))

    def on_scroll(self, nx: float, ny: float, delta: int) -> None:
        self._wheel_acc += delta
        self._wheel_pos = (nx, ny)
        if not self._wheel_timer.isActive():
            self._wheel_timer.start()

    def _flush_wheel(self) -> None:
        delta, self._wheel_acc = self._wheel_acc, 0
        if not delta:
            return
        nx, ny = self._wheel_pos
        # 滚轮一格（120）大约滑动 15% 屏幕高度，最多 60%
        dist = max(min(delta / 120 * 0.15, 0.6), -0.6)
        y2 = min(max(ny + dist, 0.02), 0.98)
        x, y = self._pt(nx, ny)
        _, ty = self._pt(nx, y2)
        self._run(lambda w: w.swipe(x, y, x, ty, duration_ms=300))

    def go_home(self) -> None:
        self._run(lambda w: w.home())

    def app_switcher(self) -> None:
        w_, h_ = self.points.width, self.points.height
        self._run(lambda w: w.swipe(w_ * 0.5, h_ - 2, w_ * 0.5, h_ * 0.6, duration_ms=300, hold_ms=700))

    def notification_center(self) -> None:
        w_, h_ = self.points.width, self.points.height
        self._run(lambda w: w.swipe(w_ * 0.25, 2, w_ * 0.25, h_ * 0.7, duration_ms=300))

    def control_center(self) -> None:
        w_, h_ = self.points.width, self.points.height
        self._run(lambda w: w.swipe(w_ * 0.9, 2, w_ * 0.9, h_ * 0.6, duration_ms=300))

    def toggle_lock(self) -> None:
        def action(w: WDAClient) -> None:
            if w.is_locked():
                w.unlock()
            else:
                w.lock()
        self._run(action)

    # ------------------------------------------------------------------ 文字

    def queue_text(self, text: str) -> None:
        # 合并快速连续的按键，减少请求次数
        self._pending_text += text
        self._text_timer.start()

    def _flush_text(self) -> None:
        text, self._pending_text = self._pending_text, ""
        if text:
            self._run(lambda w: w.type_text(text))

    def send_text_input(self) -> None:
        text = self.text_input.text()
        if text:
            self.text_input.clear()
            self._run(lambda w: w.type_text(text), "已发送文字")

    def paste_pc_clipboard(self) -> None:
        text = QGuiApplication.clipboard().text()
        if text:
            self._run(lambda w: w.type_text(text), "已输入剪贴板文字")
        else:
            self._status("电脑剪贴板里没有文字")

    def push_clipboard(self) -> None:
        text = QGuiApplication.clipboard().text()
        if not text:
            self._status("电脑剪贴板里没有文字")
            return
        self._run(lambda w: w.set_pasteboard(text), "已写入手机剪贴板，可在手机上长按粘贴")

    def pull_clipboard(self) -> None:
        wda = self.wda
        if wda is None:
            return

        def done(text: str) -> None:
            QGuiApplication.clipboard().setText(text)
            self._status(f"已复制手机剪贴板（{len(text)} 字）" if text else "手机剪贴板为空")

        self._start_task(wda.get_pasteboard, done,
                         lambda m: self._status(f"读取手机剪贴板失败（iOS 可能限制后台读取）：{m}", error=True))

    # ------------------------------------------------------------------ 截图 / 照片

    def _pictures_dir(self) -> str:
        base = QStandardPaths.writableLocation(QStandardPaths.StandardLocation.PicturesLocation)
        path = os.path.join(base or os.path.expanduser("~"), "iPhone Link")
        os.makedirs(path, exist_ok=True)
        return path

    def save_screenshot(self) -> None:
        wda = self.wda
        if wda is None:
            return
        path = os.path.join(self._pictures_dir(), time.strftime("iPhone_%Y%m%d_%H%M%S.png"))

        def work():
            with open(path, "wb") as f:
                f.write(wda.screenshot_png())
            return path

        self._start_task(work, lambda p: self._status(f"截图已保存：{p}"))

    def export_photos(self) -> None:
        if self.session is None:
            QMessageBox.information(self, "提示", "导出照片需要通过 USB 连接（直连模式不支持）。")
            return
        dest = QFileDialog.getExistingDirectory(self, "选择照片保存位置", self._pictures_dir())
        if not dest:
            return
        udid = self.session.device.udid
        self._status("正在导出照片，照片多时需要较长时间…")
        self.log_dock.show()
        self._start_task(lambda: export_photos(udid, dest, self.log),
                         lambda _: (self._status(f"照片已导出到 {dest}"),
                                    QDesktopServices.openUrl(QUrl.fromLocalFile(dest))))

    # ------------------------------------------------------------------ 退出

    def closeEvent(self, e) -> None:
        self.disconnect_device()
        for t in list(self._tasks):
            t.wait(2000)
        super().closeEvent(e)


def main() -> int:
    QGuiApplication.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)
    app = QApplication(sys.argv)
    app.setApplicationName("iPhone Link")
    app.setWindowIcon(QIcon.fromTheme("phone"))
    win = MainWindow()
    win.show()
    return app.exec()
