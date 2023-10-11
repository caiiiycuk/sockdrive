//
// Created by caiii on 30.08.2023.
//
#include <cstring>

#include "../include/sockdrive.h"
#include "emscripten.h"
#include "../lz4/lz4.h"

const char *jsImpl = 
#include "dist/sockdriveNative.js"
;

EM_ASYNC_JS(size_t, em_sockdrive_open, (const char* url, 
    const char* owner, const char* name, const char* token,
    const char* jsImpl), {
    url = UTF8ToString(url);
    owner = UTF8ToString(owner);
    name = UTF8ToString(name);
    token = UTF8ToString(token);

    if (!Module.sockdrive) {
        jsImpl = UTF8ToString(jsImpl);
        eval(jsImpl);
        Module.sockdrive.onOpen =  (drive, read, write) => {
            Module.log("sockdrive: " + drive + ", read=" + read + ", write=" + write);
        };
        Module.sockdrive.onError = (e) => {
            Module.err(e.message ?? "unable to open sockdrive");
        };
    }

    try {
        return await Module.sockdrive.open(url, owner, name, token.length > 0 ? token : Module.token)
    } catch (e) {
        Module.err(e.message ?? "sockdrive not connected");
        return 0;
    }
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

EM_JS(uint32_t, sockdrive_size, (size_t handle), {
    return Module.sockdrive.size(handle);
});

EM_JS(uint32_t, sockdrive_heads, (size_t handle), {
    return Module.sockdrive.heads(handle);
});

EM_JS(uint32_t, sockdrive_sectors, (size_t handle), {
    return Module.sockdrive.sectors(handle);
});

EM_JS(uint32_t, sockdrive_sector_size, (size_t handle), {
    return Module.sockdrive.sector_size(handle);
});

EM_JS(uint32_t, sockdrive_cylinders, (size_t handle), {
    return Module.sockdrive.cylinders(handle);
});

size_t sockdrive_open(const char* url, 
    const char* owner, const char* name, const char* token) {
    return em_sockdrive_open(url, owner, name, token, jsImpl);
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