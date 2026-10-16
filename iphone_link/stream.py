"""读取 WDA 的 MJPEG 画面流（默认手机端口 9100），逐帧解码成 QImage。

MJPEG 流不可用时自动退回到轮询截图接口（较慢，但总能看到画面）。
"""

from __future__ import annotations

import time

import requests
from PySide6.QtCore import QThread, Signal
from PySide6.QtGui import QImage

from .wda import WDAClient, WDAError

SOI = b"\xff\xd8"
EOI = b"\xff\xd9"


def iter_mjpeg_frames(chunks, max_buffer: int = 8 * 1024 * 1024):
    """从 MJPEG 字节流中切出完整的 JPEG 帧。

    优先使用每帧头部的 Content-Length；没有时按 JPEG 起止标记切分。
    """
    buf = b""
    for chunk in chunks:
        if not chunk:
            continue
        buf += chunk
        while True:
            start = buf.find(SOI)
            if start == -1:
                # 保留帧头（里面有 Content-Length）；只在缓冲异常变大时丢弃
                if len(buf) > 64 * 1024:
                    buf = buf[-1:]
                break
            length = _content_length(buf[:start])
            if length is not None:
                if len(buf) < start + length:
                    break
                frame = buf[start:start + length]
                buf = buf[start + length:]
                yield frame
                continue
            end = buf.find(EOI, start + 2)
            if end == -1:
                if len(buf) > max_buffer:
                    buf = b""
                break
            frame = buf[start:end + 2]
            buf = buf[end + 2:]
            yield frame


def _content_length(header: bytes) -> int | None:
    # 只看紧挨着这帧的那段头部
    for line in reversed(header.split(b"\n")):
        line = line.strip()
        if line.lower().startswith(b"content-length:"):
            try:
                return int(line.split(b":", 1)[1].strip())
            except ValueError:
                return None
    return None


class FrameReader(QThread):
    frame = Signal(QImage)
    status = Signal(str)

    def __init__(self, mjpeg_url: str, wda: WDAClient, parent=None):
        super().__init__(parent)
        self.mjpeg_url = mjpeg_url
        self.wda = wda
        self._running = True
        self._resp: requests.Response | None = None

    def stop(self) -> None:
        self._running = False
        resp = self._resp
        if resp is not None:
            try:
                resp.close()
            except Exception:
                pass
        self.wait(3000)

    def run(self) -> None:
        failures = 0
        while self._running:
            got_frame = self._run_mjpeg()
            if not self._running:
                break
            failures = 0 if got_frame else failures + 1
            if failures >= 3:
                self.status.emit("画面流不可用，改用截图模式（较慢）")
                self._run_screenshot_polling()
                failures = 0
            else:
                time.sleep(0.5)

    def _run_mjpeg(self) -> bool:
        got_frame = False
        http = requests.Session()
        http.trust_env = False
        try:
            self._resp = http.get(self.mjpeg_url, stream=True, timeout=(3, 10))
            self._resp.raise_for_status()
            for data in iter_mjpeg_frames(self._resp.iter_content(chunk_size=64 * 1024)):
                if not self._running:
                    break
                img = QImage.fromData(data, "JPG")
                if not img.isNull():
                    got_frame = True
                    self.frame.emit(img)
        except requests.RequestException:
            pass
        finally:
            if self._resp is not None:
                self._resp.close()
            self._resp = None
            http.close()
        return got_frame

    def _run_screenshot_polling(self) -> None:
        last_retry = time.time()
        while self._running:
            try:
                img = QImage.fromData(self.wda.screenshot_png(), "PNG")
                if not img.isNull():
                    self.frame.emit(img)
            except WDAError as e:
                self.status.emit(str(e))
                time.sleep(1.0)
            # 每 15 秒试一次能否恢复 MJPEG
            if time.time() - last_retry > 15:
                return
