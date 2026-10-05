#!/bin/sh
# Headless engine plumbing: Xvfb for rendering, a session bus so gtk_init
# does not block on a missing D-Bus (the CI lesson), WebKit sandbox off
# (a container IS the sandbox).
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export GDK_PLATFORM=x11
Xvfb :99 -screen 0 1280x800x24 &
export DISPLAY=:99
exec dbus-run-session -- /usr/local/bin/navette "$@"
