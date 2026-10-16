"""显示手机画面的控件，并把鼠标/键盘操作翻译成手势信号。

坐标统一用 0~1 的归一化值发出，由主窗口换算成 WDA 的 point 坐标。

鼠标操作：
  单击            → 点按
  按住不动 ≥0.45s → 长按
  拖动            → 按真实轨迹滑动（可用于翻页、下拉刷新）
  先按住再拖动    → 长按拖动（移动图标等）
  滚轮            → 上下滑动
键盘操作（先点一下画面让它获得焦点）：
  直接打字（支持中文输入法） → 输入到手机当前输入框
  Ctrl+V                     → 把电脑剪贴板里的文字输入到手机
"""

from __future__ import annotations

import time

from PySide6.QtCore import QPointF, QRect, QRectF, Qt, Signal
from PySide6.QtGui import QColor, QImage, QInputMethodEvent, QKeyEvent, QPainter
from PySide6.QtWidgets import QWidget

TAP_SLOP_PX = 6
LONG_PRESS_MS = 450
HOLD_BEFORE_DRAG_MS = 350
MIN_SEGMENT_MS = 25


class ScreenView(QWidget):
    tapped = Signal(float, float)
    long_pressed = Signal(float, float, int)
    dragged = Signal(list, int)          # [(nx, ny, dt_ms), ...], 起始按住毫秒
    scrolled = Signal(float, float, int)  # nx, ny, 滚轮 delta
    text_entered = Signal(str)
    paste_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self._image: QImage | None = None
        self._target = QRectF()
        self._placeholder = "未连接"
        self._press: list[tuple[float, float, float, QPointF]] | None = None
        self.setMinimumSize(240, 420)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
        self.setAttribute(Qt.WidgetAttribute.WA_InputMethodEnabled, True)
        self.setAttribute(Qt.WidgetAttribute.WA_OpaquePaintEvent, True)
        self.setMouseTracking(False)

    # ------------------------------------------------------------------ 画面

    def set_placeholder(self, text: str) -> None:
        self._placeholder = text
        self._image = None
        self.update()

    def set_image(self, img: QImage) -> None:
        self._image = img
        self.update()

    def image_size(self):
        return self._image.size() if self._image is not None else None

    def paintEvent(self, event) -> None:
        p = QPainter(self)
        p.fillRect(self.rect(), QColor(24, 24, 27))
        if self._image is None or self._image.isNull():
            self._target = QRectF()
            p.setPen(QColor(160, 160, 170))
            p.drawText(self.rect(), Qt.AlignmentFlag.AlignCenter, self._placeholder)
            return
        iw, ih = self._image.width(), self._image.height()
        scale = min(self.width() / iw, self.height() / ih)
        w, h = iw * scale, ih * scale
        self._target = QRectF((self.width() - w) / 2, (self.height() - h) / 2, w, h)
        p.setRenderHint(QPainter.RenderHint.SmoothPixmapTransform, True)
        p.drawImage(self._target, self._image)
        if self.hasFocus():
            p.setPen(QColor(59, 130, 246))
            p.drawRect(self.rect().adjusted(0, 0, -1, -1))

    def _normalize(self, pos: QPointF) -> tuple[float, float] | None:
        if self._target.isEmpty():
            return None
        nx = (pos.x() - self._target.x()) / self._target.width()
        ny = (pos.y() - self._target.y()) / self._target.height()
        return min(max(nx, 0.0), 1.0), min(max(ny, 0.0), 1.0)

    def _inside(self, pos: QPointF) -> bool:
        return self._target.contains(pos)

    # ------------------------------------------------------------------ 鼠标

    def mousePressEvent(self, e) -> None:
        self.setFocus()
        if e.button() != Qt.MouseButton.LeftButton or not self._inside(e.position()):
            self._press = None
            return
        nx, ny = self._normalize(e.position())
        self._press = [(nx, ny, time.monotonic(), e.position())]

    def mouseMoveEvent(self, e) -> None:
        if self._press is None:
            return
        norm = self._normalize(e.position())
        if norm is not None:
            self._press.append((norm[0], norm[1], time.monotonic(), e.position()))

    def mouseReleaseEvent(self, e) -> None:
        if self._press is None or e.button() != Qt.MouseButton.LeftButton:
            return
        norm = self._normalize(e.position())
        if norm is not None:
            self._press.append((norm[0], norm[1], time.monotonic(), e.position()))
        pts, self._press = self._press, None
        self._emit_gesture(pts)

    def _emit_gesture(self, pts) -> None:
        x0, y0, t0, p0 = pts[0]
        t_end = pts[-1][2]
        first_far = next(
            (i for i, (_, _, _, p) in enumerate(pts)
             if (p - p0).manhattanLength() > TAP_SLOP_PX),
            None,
        )
        if first_far is None:
            duration = int((t_end - t0) * 1000)
            if duration >= LONG_PRESS_MS:
                self.long_pressed.emit(x0, y0, duration)
            else:
                self.tapped.emit(x0, y0)
            return

        # 起点到开始移动之前停留的时间：超过阈值视为“长按后拖动”
        hold_ms = int((pts[first_far - 1][2] - t0) * 1000)
        hold_ms = hold_ms if hold_ms >= HOLD_BEFORE_DRAG_MS else 0

        # 降采样，保留真实速度（松手时的速度决定惯性滑动）
        path = [(x0, y0, 0)]
        last_t = pts[first_far - 1][2] if hold_ms else t0
        for x, y, t, _ in pts[first_far:-1]:
            if (t - last_t) * 1000 >= MIN_SEGMENT_MS:
                path.append((x, y, int((t - last_t) * 1000)))
                last_t = t
        x, y, t, _ = pts[-1]
        path.append((x, y, max(int((t - last_t) * 1000), 10)))
        self.dragged.emit(path, hold_ms)

    def wheelEvent(self, e) -> None:
        norm = self._normalize(e.position())
        delta = e.angleDelta().y()
        if norm is not None and delta:
            self.scrolled.emit(norm[0], norm[1], delta)

    # ------------------------------------------------------------------ 键盘

    def keyPressEvent(self, e: QKeyEvent) -> None:
        key = e.key()
        mods = e.modifiers()
        if key == Qt.Key.Key_V and mods & Qt.KeyboardModifier.ControlModifier:
            self.paste_requested.emit()
            return
        if key == Qt.Key.Key_Backspace:
            self.text_entered.emit("\b")
            return
        if key in (Qt.Key.Key_Return, Qt.Key.Key_Enter):
            self.text_entered.emit("\n")
            return
        if key == Qt.Key.Key_Tab:
            self.text_entered.emit("\t")
            return
        text = e.text()
        if text and text.isprintable() and not (mods & (Qt.KeyboardModifier.ControlModifier
                                                         | Qt.KeyboardModifier.AltModifier)):
            self.text_entered.emit(text)
            return
        super().keyPressEvent(e)

    def inputMethodEvent(self, e: QInputMethodEvent) -> None:
        # 中文等输入法上屏的文字
        if e.commitString():
            self.text_entered.emit(e.commitString())
        e.accept()

    def inputMethodQuery(self, query):
        if query == Qt.InputMethodQuery.ImEnabled:
            return True
        if query == Qt.InputMethodQuery.ImCursorRectangle:
            # 让输入法候选框出现在画面下方
            return QRect(int(self.width() / 2), int(self.height() * 0.8), 1, 20)
        return super().inputMethodQuery(query)
