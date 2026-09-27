@echo off
rem SPDX-License-Identifier: Apache-2.0
setlocal

set "LD_PRELOAD="
set "LD_AUDIT="
set "LD_LIBRARY_PATH="
set "DYLD_INSERT_LIBRARIES="
set "DYLD_LIBRARY_PATH="
set "DYLD_FRAMEWORK_PATH="
set "VK_DRIVER_FILES="
set "VK_ADD_DRIVER_FILES="
set "VK_ICD_FILENAMES="
set "VK_LAYER_PATH="
set "VK_ADD_LAYER_PATH="
set "VK_INSTANCE_LAYERS="
set "VK_LOADER_LAYERS_ENABLE="
set "VK_LOADER_LAYERS_DISABLE="
set "VK_LOADER_DRIVERS_SELECT="
set "VK_LOADER_DRIVERS_DISABLE="
set "GALAXY_BH2D_CLEAN_LAUNCH=1"

where py >nul 2>nul
if errorlevel 1 goto :python_fallback
py -3 -I "%~dp0bench-bh2d-hardware.py" %*
exit /b %ERRORLEVEL%

:python_fallback
where python >nul 2>nul
if %ERRORLEVEL% NEQ 0 (
  echo BH #2D hardware sweep: Python 3 is required 1>&2
  exit /b 127
)
python -I "%~dp0bench-bh2d-hardware.py" %*
exit /b %ERRORLEVEL%
