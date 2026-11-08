#!/bin/sh
# Compiles the helpers (oracle.cc, records.cc, and odin.cc) against the static libvalhalla in the dev image.
set -e
cd /w
for prog in oracle records odin; do
	g++ -std=c++20 -O1 -w "$prog.cc" -o "$prog" \
		$(pkg-config --cflags libvalhalla) \
		/usr/local/lib/libvalhalla.a \
		-lz -lprotobuf-lite -lprime_server -lzmq -lspatialite -lsqlite3 -lluajit-5.1 -lgeos_c -lcurl -lssl -lcrypto -ltiff -llz4 -lpthread 2>&1 | head -40
	ls -la "$prog"
done
