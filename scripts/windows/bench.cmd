@echo off
cd /d "%~dp0"
echo egui DX12...
postman.exe --demo --bench --gpu dx12 >> bench.txt
echo egui Vulkan...
postman.exe --demo --bench --gpu vulkan >> bench.txt
echo gpui...
postman-gpui.exe --demo 50000 --bench >> bench.txt
type bench.txt
pause
