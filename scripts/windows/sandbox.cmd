@echo off
rem Grant ALL APPLICATION PACKAGES read access to this folder (like Program Files) for the AppContainer sandbox.
icacls "%~dp0." /grant "*S-1-15-2-1:(OI)(CI)(RX)" /T
pause
