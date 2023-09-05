//
// Created by caiii on 30.08.2023.
//
#include "../sockdrive.h"
#include "emscripten.h"

const char *jsLruImpl =
#include "js-lru.js"
;

EM_ASYNC_JS(size_t, em_sockdrive_open, (const char* host, uint16_t port, const char* lruImpl), {
    host = UTF8ToString(host);

    if (!Module.sockdrive) {
        if (!Module.LruMap) {
            lruImpl = UTF8ToString(lruImpl);
            eval(lruImpl);
            Module.LruMap = lru_map.LRUMap;
        }

        const memroyLimit = 32 * 1024 * 104;
        const sectorSize = 512;
        const aheadRange = 64 * 1024 / sectorSize;
        Module.sockdrive = {
            seq: 0,
            memroyLimit,
            aheadRange,
            sectorSize,
            readBuffer: new Uint8Array(1 + 4 + 1),
            writeBuffer: new Uint8Array(1 + 4 + sectorSize),
            aheadBuffer: new Uint8Array(sectorSize * aheadRange),
            aheadBufferPos: 0,
            resolveRead: null,
            readPtr: null,
            readStartedAt: null,
            readSector: 0,
            readHandle: 0,
            drives: {},
            stats: {
                read: 0,
                write: 0,
                readTotalTime: 0,
            },
        }
    };
    
    const url = "ws://" + host + ":" + port;
    return new Promise((resolve) => {
        const socket = new WebSocket(url);
        socket.binaryType = "arraybuffer";
        const errorListener = () => {
            console.error("Unable connect to " + url);
            resolve(0);
        };
        socket.addEventListener("open", () => {
            socket.removeEventListener("error", errorListener);
            socket.addEventListener("error", (event) => {
                console.error("WebSocket error", event);
                if (Module.sockdrive.resolveRead !== null) {
                    Module.sockdrive.resolveRead(1);
                    Module.sockdrive.resolveRead = null;
                    Module.sockdrive.readPtr = 0;
                }
            });
            Module.sockdrive.seq++;
            Module.sockdrive.drives[Module.sockdrive.seq] = {
                socket,
                cache: new Module.LruMap(Math.floor(Module.sockdrive.memroyLimit / Module.sockdrive.sectorSize)),
            };
            resolve(Module.sockdrive.seq);
        });
        socket.addEventListener("message", (event) => {
            if (event.data instanceof ArrayBuffer) {
                if (Module.sockdrive.resolveRead === null || Module.sockdrive.readPtr === null) {
                    console.error("sockdrive received unexcepted message");
                } else if (Module.sockdrive.aheadBufferPos < Module.sockdrive.aheadBuffer.length) {
                    if (event.data.byteLength > Module.sockdrive.aheadBuffer.length - Module.sockdrive.aheadBufferPos) {
                        console.error("sockdrive wrong message len " + event.data.byteLength + " instead of " + (Module.sockdrive.aheadBuffer.length - Module.sockdrive.aheadBufferPos));
                    } else {
                        Module.sockdrive.aheadBuffer.set(new Uint8Array(event.data), Module.sockdrive.aheadBufferPos);
                        Module.sockdrive.aheadBufferPos += event.data.byteLength;

                        if (Module.sockdrive.aheadBufferPos == Module.sockdrive.aheadBuffer.length) {
                            const cache = Module.sockdrive.drives[Module.sockdrive.readHandle].cache;
                            const sector = Module.sockdrive.readSector;
                            for (let i = 0; i < Module.sockdrive.aheadRange; ++i) {
                                if (cache.get(sector + i) === undefined) {
                                    cache.set(sector + i, Module.sockdrive.aheadBuffer.slice(i * Module.sockdrive.sectorSize, (i + 1) * Module.sockdrive.sectorSize));   
                                }
                            }
                            
                            Module.HEAPU8.set(new Uint8Array(Module.sockdrive.aheadBuffer.slice(0, Module.sockdrive.sectorSize)), Module.sockdrive.readPtr);
                            Module.sockdrive.resolveRead(0);
                            Module.sockdrive.resolveRead = null;
                            Module.sockdrive.readPtr = null;
                            Module.sockdrive.stats.read += Module.sockdrive.sectorSize * Module.sockdrive.aheadRange;
                            Module.sockdrive.stats.readTotalTime += Date.now() - Module.sockdrive.readStartedAt;
                        }
                    }
                }
            } else {
                console.error("sockdrive received unknown message");
            }
        });
    });
});

EM_ASYNC_JS(uint8_t, sockdrive_read, (size_t handle, uint32_t sector, uint8_t * buffer), {
    return new Promise((resolve) => {
        if (!Module.sockdrive) {
            console.error("sockdrive is not initiaized");
            resolve(1);
        } else if (Module.sockdrive.resolveRead !== null || Module.sockdrive.readPtr !== null) {
            console.error("sockdrive is in read opration alread (this should not happen)");
            resolve(1);
        } else {
            const { socket, cache } = Module.sockdrive.drives[handle];
            if (!socket) {
                console.error("not a sockdrive handle");
                resolve(1);
           } else {
                const cached = cache.get(sector);
                if (cached) {
                    Module.HEAPU8.set(cached, buffer);
                    resolve(0);
                } else {
                    Module.sockdrive.readStartedAt = Date.now();
                    Module.sockdrive.resolveRead = resolve;
                    Module.sockdrive.readPtr = buffer;
                    Module.sockdrive.readSector = sector;
                    Module.sockdrive.readHandle = handle;
                    Module.sockdrive.aheadBufferPos = 0;

                    Module.sockdrive.readBuffer[0] = 1; // read
                    Module.sockdrive.readBuffer[1] = sector & 0xFF;
                    Module.sockdrive.readBuffer[2] = (sector >> 8) & 0xFF;
                    Module.sockdrive.readBuffer[3] = (sector >> 16) & 0xFF;
                    Module.sockdrive.readBuffer[4] = (sector >> 24) & 0xFF;
                    Module.sockdrive.readBuffer[5] = Module.sockdrive.aheadRange;
                    socket.send(Module.sockdrive.readBuffer.buffer);
                }
           }
        }
    });
});

EM_ASYNC_JS(uint8_t, sockdrive_write, (size_t handle, uint32_t sector, uint8_t * buffer), {
    return new Promise((resolve) => {
        if (!Module.sockdrive) {
            console.error("sockdrive is not initiaized");
            resolve(1);
        } else {
            const { socket, cache } = Module.sockdrive.drives[handle];
            if (!socket) {
                console.error("not a sockdrive handle");
            } else {
                Module.sockdrive.stats.write += Module.sockdrive.sectorSize;
                Module.sockdrive.writeBuffer[0] = 2; // write
                Module.sockdrive.writeBuffer[1] = sector & 0xFF;
                Module.sockdrive.writeBuffer[2] = (sector >> 8) & 0xFF;
                Module.sockdrive.writeBuffer[3] = (sector >> 16) & 0xFF;
                Module.sockdrive.writeBuffer[4] = (sector >> 24) & 0xFF;
                // TBD: maybe do not copy and send just slice ???
                const sectorBuffer = Module.HEAPU8.slice(buffer, buffer + Module.sockdrive.sectorSize);
                Module.sockdrive.writeBuffer.set(sectorBuffer, 5);
                socket.send(Module.sockdrive.writeBuffer.buffer);
                cache.set(sector, sectorBuffer);

                resolve(0);
            }
        }
    });
});

EM_JS(void, sockdrive_close, (size_t handle), {
    if (Module.sockdrive && Module.sockdrive.drives[handle]) {
        Module.sockdrive.drives[handle].socket.close();
        delete Module.sockdrive.drives[handle];
    }
});

size_t sockdrive_open(const char* host, uint16_t port) {
    return em_sockdrive_open(host, port, jsLruImpl);
}