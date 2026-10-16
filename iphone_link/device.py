"""通过 pymobiledevice3 命令行管理 iPhone：列出设备、挂载开发者镜像、
启动 WebDriverAgent、建立 USB 端口转发、导出照片。

这里刻意调用 `python -m pymobiledevice3` 命令行而不是导入它的 Python API：
命令行接口比内部 API 稳定得多，升级 pymobiledevice3 时不容易坏。
iOS 17+ 需要的隧道由新版 pymobiledevice3 自动建立（userspace 模式，无需管理员权限）。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass

from .wda import WDAClient

DEFAULT_WDA_BUNDLE = "com.facebook.WebDriverAgentRunner.xctrunner"

LogFn = Callable[[str], None]


@dataclass
class DeviceInfo:
    udid: str
    name: str
    ios_version: str
    product_type: str
    connection: str

    @property
    def ios_major(self) -> int:
        try:
            return int(self.ios_version.split(".")[0])
        except (ValueError, IndexError):
            return 0

    def label(self) -> str:
        return f"{self.name}（iOS {self.ios_version}，{self.product_type}，{self.connection}）"


def _python_exe() -> str:
    # 用 pythonw.exe 启动本程序时，子进程改用同目录的 python.exe，否则拿不到命令输出
    exe = sys.executable
    if os.path.basename(exe).lower() == "pythonw.exe":
        candidate = os.path.join(os.path.dirname(exe), "python.exe")
        if os.path.exists(candidate):
            return candidate
    return exe


def _pmd_cmd(*args: str) -> list[str]:
    return [_python_exe(), "-m", "pymobiledevice3", *args]


def _popen_kwargs() -> dict:
    kwargs: dict = {
        "stdout": subprocess.PIPE,
        "stderr": subprocess.STDOUT,
        "stdin": subprocess.DEVNULL,
        "env": {**os.environ, "PYTHONIOENCODING": "utf-8", "PYTHONUTF8": "1"},
    }
    if sys.platform == "win32":
        # 不弹出黑色控制台窗口
        kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW
    return kwargs


def run_pmd(*args: str, timeout: float = 120) -> str:
    """运行一次性 pymobiledevice3 命令，返回输出；失败抛出 RuntimeError。"""
    kwargs = _popen_kwargs()
    kwargs.pop("stdin")
    try:
        proc = subprocess.run(_pmd_cmd(*args), timeout=timeout, text=True,
                              encoding="utf-8", errors="replace", **kwargs)
    except subprocess.TimeoutExpired as e:
        raise RuntimeError(f"命令超时：pymobiledevice3 {' '.join(args)}") from e
    if proc.returncode != 0:
        tail = (proc.stdout or "").strip()[-1500:]
        raise RuntimeError(f"pymobiledevice3 {' '.join(args)} 失败：\n{tail}")
    return proc.stdout or ""


def _parse_json(output: str):
    """从命令输出中取出 JSON（输出前面可能夹杂日志行）。"""
    for start_char in ("[", "{"):
        idx = output.find(start_char)
        while idx != -1:
            try:
                return json.loads(output[idx:])
            except ValueError:
                idx = output.find(start_char, idx + 1)
    raise RuntimeError(f"无法解析输出：{output[-500:]}")


def list_devices() -> list[DeviceInfo]:
    data = _parse_json(run_pmd("usbmux", "list", timeout=30))
    devices = []
    for d in data:
        if not isinstance(d, dict):
            continue
        devices.append(DeviceInfo(
            udid=d.get("UniqueDeviceID") or d.get("Identifier") or "",
            name=d.get("DeviceName") or "iPhone",
            ios_version=d.get("ProductVersion") or "?",
            product_type=d.get("ProductType") or "?",
            connection=d.get("ConnectionType") or "USB",
        ))
    return devices


def find_wda_bundle(udid: str) -> str | None:
    """在已安装的应用里找 WebDriverAgentRunner（用免费 Apple ID 签名后包名可能被改过）。"""
    data = _parse_json(run_pmd("apps", "list", "--type", "User", "--udid", udid, timeout=60))
    candidates = [bid for bid in data if "webdriveragentrunner" in bid.lower()]
    if not candidates:
        return None
    candidates.sort(key=lambda b: (not b.endswith(".xctrunner"), b))
    return candidates[0]


class _ManagedProcess:
    """一个持续运行的子进程，把输出转发到日志。"""

    def __init__(self, name: str, args: list[str], log: LogFn):
        self.name = name
        self.log = log
        self.proc = subprocess.Popen(_pmd_cmd(*args), text=True, encoding="utf-8",
                                     errors="replace", bufsize=1, **_popen_kwargs())
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            line = line.rstrip()
            if line:
                self.log(f"[{self.name}] {line}")
        self.log(f"[{self.name}] 进程已退出（代码 {self.proc.poll()}）")

    def alive(self) -> bool:
        return self.proc.poll() is None

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()


class DeviceSession:
    """一次连接：启动 WDA + 端口转发，断开时清理所有子进程。"""

    def __init__(self, device: DeviceInfo, log: LogFn,
                 wda_port: int = 8100, mjpeg_port: int = 9100,
                 bundle_id: str | None = None):
        self.device = device
        self.log = log
        self.wda_port = wda_port
        self.mjpeg_port = mjpeg_port
        self.bundle_id = bundle_id
        self._procs: list[_ManagedProcess] = []

    @property
    def wda_url(self) -> str:
        return f"http://127.0.0.1:{self.wda_port}"

    @property
    def mjpeg_url(self) -> str:
        return f"http://127.0.0.1:{self.mjpeg_port}"

    def start(self, progress: LogFn, ready_timeout: float = 90) -> WDAClient:
        udid = self.device.udid
        wda = WDAClient(self.wda_url)

        progress("建立 USB 端口转发…")
        self._procs.append(_ManagedProcess(
            "转发WDA", ["usbmux", "forward", "--udid", udid, str(self.wda_port), "8100"], self.log))
        self._procs.append(_ManagedProcess(
            "转发画面", ["usbmux", "forward", "--udid", udid, str(self.mjpeg_port), "9100"], self.log))
        time.sleep(1.0)

        # WDA 已经在手机上运行（比如上次没退出），就不用再启动了
        if wda.is_ready():
            progress("检测到 WDA 已在运行")
            return wda

        progress("挂载开发者磁盘镜像（首次需要下载，可能较慢）…")
        try:
            out = run_pmd("mounter", "auto-mount", "--udid", udid, timeout=600)
            for line in out.strip().splitlines()[-3:]:
                self.log(f"[挂载] {line}")
        except RuntimeError as e:
            # 已挂载等情况也可能返回非 0，继续尝试启动 WDA
            self.log(f"[挂载] {e}")

        bundle = self.bundle_id
        if not bundle:
            progress("查找手机上的 WebDriverAgent…")
            try:
                bundle = find_wda_bundle(udid)
            except RuntimeError as e:
                self.log(f"[查找] {e}")
            if not bundle:
                raise RuntimeError(
                    "手机上没有找到 WebDriverAgentRunner。\n"
                    "请先按 README 的步骤用 Sideloadly 把 WDA 安装到手机，"
                    "或在“设置”里手动填写它的 Bundle ID。")
        self.log(f"[查找] 使用 WDA：{bundle}")

        progress("启动 WebDriverAgent…")
        self._procs.append(_ManagedProcess(
            "WDA", ["developer", "dvt", "xcuitest", "--udid", udid, bundle], self.log))

        progress("等待 WDA 就绪…")
        deadline = time.time() + ready_timeout
        while time.time() < deadline:
            if wda.is_ready():
                progress("WDA 已就绪")
                return wda
            if not self._procs[-1].alive():
                raise RuntimeError(
                    "WDA 启动失败，请查看日志。常见原因：\n"
                    "1. 手机未开启“开发者模式”（设置 → 隐私与安全性 → 开发者模式）\n"
                    "2. 未信任开发者证书（设置 → 通用 → VPN 与设备管理）\n"
                    "3. 免费 Apple ID 签名已超过 7 天，需要用 Sideloadly 重新安装")
            time.sleep(1.0)
        raise RuntimeError("等待 WDA 超时，请查看日志。手机首次运行时可能需要解锁屏幕。")

    def stop(self) -> None:
        for p in reversed(self._procs):
            p.stop()
        self._procs.clear()


def export_photos(udid: str, dest_dir: str, log: LogFn) -> None:
    """把手机相册（/DCIM）完整复制到电脑。"""
    os.makedirs(dest_dir, exist_ok=True)
    proc = _ManagedProcess("导出照片", ["afc", "pull", "--ignore-errors", "--udid", udid, "/DCIM", dest_dir], log)
    proc.proc.wait()
    proc._reader.join(timeout=2)
    if proc.proc.returncode != 0:
        raise RuntimeError("导出照片失败，请查看日志")
