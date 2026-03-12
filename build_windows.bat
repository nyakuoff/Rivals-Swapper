@echo off
REM Build Rivals Swapper for Windows
REM Outputs to: dist\RivalsSwapper\

echo Building Rivals Swapper...
.venv\Scripts\pyinstaller.exe build.spec --noconfirm --clean
echo.
echo Done! Output in: dist\RivalsSwapper\
echo Copy your tools\ folder into dist\RivalsSwapper\ before distributing.
pause
