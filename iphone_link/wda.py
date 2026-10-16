"""WebDriverAgent (WDA) HTTP 客户端。

WDA 运行在 iPhone 上，通过 HTTP 接收点击、滑动、按键等指令。
这里只用到了 appium/WebDriverAgent 中稳定、常用的接口；
手势统一走 W3C Actions，兼容性最好。
"""

from __future__ import annotations

import base64
import threading
from dataclasses import dataclass

import requests


class WDAError(RuntimeError):
    pass


@dataclass
class Size:
    width: float
    height: float


class WDAClient:
    def __init__(self, base_url: str = "http://127.0.0.1:8100", timeout: float = 15.0):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self._http = requests.Session()
        # requests 会读取系统代理；本地端口转发不应走代理
        self._http.trust_env = False
        self._session_id: str | None = None
        self._lock = threading.Lock()

    # ------------------------------------------------------------------ 底层

    def _request(self, method: str, path: str, json=None, timeout: float | None = None):
        url = f"{self.base_url}{path}"
        try:
            resp = self._http.request(method, url, json=json, timeout=timeout or self.timeout)
        except requests.RequestException as e:
            raise WDAError(f"无法连接 WDA（{url}）：{e}") from e
        try:
            data = resp.json()
        except ValueError:
            data = {}
        value = data.get("value") if isinstance(data, dict) else None
        if resp.status_code >= 400 or (isinstance(value, dict) and "error" in value):
            err = value.get("error") if isinstance(value, dict) else None
            msg = value.get("message") if isinstance(value, dict) else resp.text
            raise WDAError(f"{err or resp.status_code}: {msg}")
        return data

    def _session_request(self, method: str, path: str, json=None, timeout: float | None = None):
        """带会话的请求；会话失效时自动重建一次。"""
        sid = self.session_id()
        try:
            return self._request(method, f"/session/{sid}{path}", json=json, timeout=timeout)
        except WDAError as e:
            text = str(e).lower()
            if "session" not in text:
                raise
            with self._lock:
                self._session_id = None
            sid = self.session_id()
            return self._request(method, f"/session/{sid}{path}", json=json, timeout=timeout)

    # ------------------------------------------------------------------ 会话

    def status(self, timeout: float = 3.0) -> dict:
        return self._request("GET", "/status", timeout=timeout)

    def is_ready(self) -> bool:
        try:
            self.status(timeout=2.0)
            return True
        except WDAError:
            return False

    def session_id(self) -> str:
        with self._lock:
            if self._session_id:
                return self._session_id
            data = self._request("POST", "/session", json={"capabilities": {"alwaysMatch": {}}})
            sid = data.get("sessionId") or (data.get("value") or {}).get("sessionId")
            if not sid:
                raise WDAError(f"创建会话失败：{data}")
            self._session_id = sid
            return sid

    def window_size(self) -> Size:
        value = self._session_request("GET", "/window/size")["value"]
        return Size(float(value["width"]), float(value["height"]))

    def update_settings(self, **settings) -> None:
        self._session_request("POST", "/appium/settings", json={"settings": settings})

    def configure_mjpeg(self, fps: int = 30, quality: int = 60, scale: int = 50) -> None:
        """调整画面流：帧率、JPEG 质量（1-100）、缩放比例（1-100%）。"""
        self.update_settings(
            mjpegServerFramerate=int(fps),
            mjpegServerScreenshotQuality=int(quality),
            mjpegScalingFactor=int(scale),
        )

    # ------------------------------------------------------------------ 手势

    def _perform(self, actions: list[dict]) -> None:
        body = {
            "actions": [
                {
                    "type": "pointer",
                    "id": "finger1",
                    "parameters": {"pointerType": "touch"},
                    "actions": actions,
                }
            ]
        }
        self._session_request("POST", "/actions", json=body)

    def tap(self, x: float, y: float) -> None:
        self._perform([
            {"type": "pointerMove", "duration": 0, "x": round(x), "y": round(y)},
            {"type": "pointerDown", "button": 0},
            {"type": "pause", "duration": 30},
            {"type": "pointerUp", "button": 0},
        ])

    def long_press(self, x: float, y: float, duration_ms: int = 800) -> None:
        self._perform([
            {"type": "pointerMove", "duration": 0, "x": round(x), "y": round(y)},
            {"type": "pointerDown", "button": 0},
            {"type": "pause", "duration": int(duration_ms)},
            {"type": "pointerUp", "button": 0},
        ])

    def swipe(self, x1: float, y1: float, x2: float, y2: float,
              duration_ms: int = 250, hold_ms: int = 0) -> None:
        """从 (x1,y1) 滑到 (x2,y2)；hold_ms>0 表示到终点后停留再抬起（例如打开多任务）。"""
        actions = [
            {"type": "pointerMove", "duration": 0, "x": round(x1), "y": round(y1)},
            {"type": "pointerDown", "button": 0},
            {"type": "pause", "duration": 20},
            {"type": "pointerMove", "duration": max(int(duration_ms), 10), "x": round(x2), "y": round(y2)},
        ]
        if hold_ms > 0:
            actions.append({"type": "pause", "duration": int(hold_ms)})
        actions.append({"type": "pointerUp", "button": 0})
        self._perform(actions)

    def drag_path(self, points: list[tuple[float, float, int]], hold_start_ms: int = 0) -> None:
        """按轨迹拖动。points 为 [(x, y, 距上一点的毫秒数), ...]。"""
        if not points:
            return
        x0, y0, _ = points[0]
        actions = [
            {"type": "pointerMove", "duration": 0, "x": round(x0), "y": round(y0)},
            {"type": "pointerDown", "button": 0},
        ]
        if hold_start_ms > 0:
            actions.append({"type": "pause", "duration": int(hold_start_ms)})
        for x, y, dt in points[1:]:
            actions.append({"type": "pointerMove", "duration": max(int(dt), 1), "x": round(x), "y": round(y)})
        actions.append({"type": "pointerUp", "button": 0})
        self._perform(actions)

    # ------------------------------------------------------------------ 系统按键

    def home(self) -> None:
        self._request("POST", "/wda/homescreen")

    def press_button(self, name: str) -> None:
        """name: home / volumeUp / volumeDown"""
        self._session_request("POST", "/wda/pressButton", json={"name": name})

    def lock(self) -> None:
        self._session_request("POST", "/wda/lock")

    def unlock(self) -> None:
        self._session_request("POST", "/wda/unlock")

    def is_locked(self) -> bool:
        return bool(self._session_request("GET", "/wda/locked").get("value"))

    # ------------------------------------------------------------------ 文本 / 剪贴板

    def type_text(self, text: str) -> None:
        """向当前获得焦点的输入框输入文字（需要手机键盘已弹出）。'\\b' 为退格，'\\n' 为回车。"""
        if text:
            self._session_request("POST", "/wda/keys", json={"value": list(text)})

    def set_pasteboard(self, text: str) -> None:
        content = base64.b64encode(text.encode("utf-8")).decode("ascii")
        self._session_request("POST", "/wda/setPasteboard",
                              json={"content": content, "contentType": "plaintext"})

    def get_pasteboard(self) -> str:
        value = self._session_request("POST", "/wda/getPasteboard",
                                      json={"contentType": "plaintext"})["value"]
        return base64.b64decode(value or "").decode("utf-8", errors="replace")

    # ------------------------------------------------------------------ 截图

    def screenshot_png(self) -> bytes:
        value = self._request("GET", "/screenshot", timeout=30)["value"]
        return base64.b64decode(value)
