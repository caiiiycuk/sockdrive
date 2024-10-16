//
// Created by caiii on 30.08.2023.
//

#ifndef SOCKDRIVE_H
#define SOCKDRIVE_H

#include <stddef.h>
#include <stdint.h>

extern "C" {
    size_t sockdrive_open(const char* url, const char* owner, const char* name, const char* token);
    uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t * buffer, bool async);
    uint8_t sockdrive_read_async_code(size_t handle, uint32_t sector, uint8_t * buffer);
    uint8_t sockdrive_write(size_t handle, uint32_t sector, uint8_t* buffer);
    uint32_t sockdrive_size(size_t handle);
    uint32_t sockdrive_heads(size_t handle);
    uint32_t sockdrive_sectors(size_t handle);
    uint32_t sockdrive_cylinders(size_t handle);
    uint32_t sockdrive_sector_size(size_t handle);
    void sockdrive_close(size_t handle);
}

#endif //SOCKDRIVE_H
