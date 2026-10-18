@echo off
cd /d "%~dp0"
echo Postman bench, about 5 minutes. Do not touch the windows.
echo Warm-up: creating demo mailbox...
postman.exe --demo --bench --gpu dx12 > nul
echo. >> bench.txt
type VERSION.txt >> bench.txt
echo [1/5] egui DX12
postman.exe --demo --bench --gpu dx12 >> bench.txt
echo [2/5] egui Vulkan
postman.exe --demo --bench --gpu vulkan >> bench.txt
echo [3/5] gpui
postman-gpui.exe --demo 50000 --bench >> bench.txt
echo [4/5] egui DX12, second run
postman.exe --demo --bench --gpu dx12 >> bench.txt
echo [5/5] gpui, second run
postman-gpui.exe --demo 50000 --bench >> bench.txt
type bench.txt
pause
