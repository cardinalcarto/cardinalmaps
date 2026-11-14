#!/bin/sh
# Compiles trace_attributes.cc against the static libvalhalla in the dev image.
# The helper subclasses `thor_worker_t`, so it needs the library's `ENABLE_SERVICES`
# (which declares the virtual `work`): without it, the subclass's vtable has another layout.
set -e
cd /w
g++ -std=c++20 -O1 -w -DENABLE_SERVICES trace_attributes.cc -o trace_attributes \
	$(pkg-config --cflags libvalhalla) \
	/usr/local/lib/libvalhalla.a \
	-lz -lprotobuf-lite -lprime_server -lzmq -lspatialite -lsqlite3 -lluajit-5.1 -lgeos_c -lcurl -lssl -lcrypto -ltiff /usr/lib/x86_64-linux-gnu/libgeotiff.so.5 -llz4 -lpthread 2>&1 | head -60
ls -la trace_attributes
