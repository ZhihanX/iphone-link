@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist .venv (
    echo 首次运行，正在创建虚拟环境并安装依赖……
    python -m venv .venv || goto :nopython
    .venv\Scripts\python -m pip install --upgrade pip
    .venv\Scripts\python -m pip install -r requirements.txt || goto :fail
)
start "" .venv\Scripts\pythonw -m iphone_link
exit /b 0

:nopython
echo 没有找到 Python，请先从 https://www.python.org/downloads/ 安装 Python 3.10 或更高版本，并勾选 "Add python.exe to PATH"。
pause
exit /b 1

:fail
echo 依赖安装失败，请检查网络后重新运行。
rmdir /s /q .venv
pause
exit /b 1
