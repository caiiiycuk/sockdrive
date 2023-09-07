//
// Created by caiii on 30.08.2023.
//
#include <cstring>

#include "../sockdrive.h"
#include "emscripten.h"
#include "lz4/lz4.h"

const char *jsImpl = 
#include "dist/bundle.js"
;

EM_JS(size_t, em_sockdrive_open, (const char* host, uint16_t port, const char* jsImpl), {
    host = UTF8ToString(host);

    if (!Module.sockdrive) {
        jsImpl = UTF8ToString(jsImpl);
        eval(jsImpl);
    }

    return Module.sockdrive.open(host, port);
});

EM_JS(uint8_t, em_sockdrive_read_sync, (size_t handle, uint32_t sector, uint8_t * buffer), {
    return Module.sockdrive.read(handle, sector, buffer, true);
});

EM_ASYNC_JS(uint8_t, em_sockdrive_read_async, (size_t handle, uint32_t sector, uint8_t * buffer), {
    return Module.sockdrive.read(handle, sector, buffer, false);
});

EM_JS(uint8_t, sockdrive_write, (size_t handle, uint32_t sector, uint8_t * buffer), {
    return Module.sockdrive.write(handle, sector, buffer);
});

EM_JS(void, sockdrive_close, (size_t handle), {
    Module.sockdrive.close(handle);
});

size_t sockdrive_open(const char* host, uint16_t port) {
    return em_sockdrive_open(host, port, jsImpl);
}

uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t * buffer) {
    auto status = em_sockdrive_read_sync(handle, sector, buffer);
    if (status == 255) {
        return em_sockdrive_read_async(handle, sector, buffer);
    }
    return status;
}

extern "C" int EMSCRIPTEN_KEEPALIVE decode_lz4_block(uint32_t compressedSize, uint32_t decodedSize, char *buffer) {
    if (compressedSize == decodedSize) {
        return decodedSize;
    }

    // 128 * 1024 is for 255 aheadRange (maximum possible)
    constexpr int compressedBuffer = 128 * 1024;
    static char compressed[compressedBuffer];

    if (compressedBuffer < compressedSize) {
        return -1;
    }

    memcpy(compressed, buffer, compressedSize);
    auto result = LZ4_decompress_safe(compressed, buffer, compressedSize, decodedSize);
    return result;
}