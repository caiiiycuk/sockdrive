//
// Created by caiii on 30.08.2023.
//
#include "../sockdrive.h"
#include "emscripten.h"

EM_ASYNC_JS(size_t, sockdrive_open, (const char* host, uint16_t port), {
    host = UTF8ToString(host);

    if (!Module.sockdrive) {
        Module.sockdrive = {
            id: 0,
            readBuffer: new Uint8Array(1 + 4),
            writeBuffer: new Uint8Array(1 + 4 + 512),
            resolveRead: null,
            readPtr: null,
            map: {},
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
            Module.sockdrive.id++;
            Module.sockdrive.map[Module.sockdrive.id] = socket;
            resolve(Module.sockdrive.id);
        });
        socket.addEventListener("message", (event) => {
            if (event.data instanceof ArrayBuffer) {
                if (event.data.byteLength != 512) {
                    console.error("sockdrive received wrong message");
                } else if (Module.sockdrive.resolveRead === null || Module.sockdrive.readPtr === null) {
                    console.error("sockdrive received unexcepted message");
                } else {
                    Module.HEAPU8.set(new Uint8Array(event.data), Module.sockdrive.readPtr);
                    Module.sockdrive.resolveRead(0);
                    Module.sockdrive.resolveRead = null;
                    Module.sockdrive.readPtr = null;
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
            const socket = Module.sockdrive.map[handle];
            if (!socket) {
                console.error("not a sockdrive handle");
                resolve(1);
           } else {
                Module.sockdrive.resolveRead = resolve;
                Module.sockdrive.readPtr = buffer;

                Module.sockdrive.readBuffer[0] = 1;
                Module.sockdrive.readBuffer[1] = sector & 0xFF;
                Module.sockdrive.readBuffer[2] = (sector >> 8) & 0xFF;
                Module.sockdrive.readBuffer[3] = (sector >> 16) & 0xFF;
                Module.sockdrive.readBuffer[4] = (sector >> 24) & 0xFF;
                socket.send(Module.sockdrive.readBuffer.buffer);
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
            const socket = Module.sockdrive.map[handle];
            if (!socket) {
                console.error("not a sockdrive handle");
            } else {
                Module.sockdrive.writeBuffer[0] = 2;
                Module.sockdrive.writeBuffer[1] = sector & 0xFF;
                Module.sockdrive.writeBuffer[2] = (sector >> 8) & 0xFF;
                Module.sockdrive.writeBuffer[3] = (sector >> 16) & 0xFF;
                Module.sockdrive.writeBuffer[4] = (sector >> 24) & 0xFF;
                // TBD: maybe do not copy and send just slice ???
                Module.sockdrive.writeBuffer.set(Module.HEAPU8.slice(buffer, buffer + 512), 5);
                socket.send(Module.sockdrive.writeBuffer.buffer);

                resolve(0);
            }
        }
    });
});

EM_JS(void, sockdrive_close, (size_t handle), {
    if (Module.sockdrive) {
        const socket = Module.sockdrive.map[handle];
        if (socket) {
            socket.close();
        }
    }
});
