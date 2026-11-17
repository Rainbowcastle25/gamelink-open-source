@echo off
if defined ZIG (
  "%ZIG%" dlltool %*
) else (
  zig dlltool %*
)
