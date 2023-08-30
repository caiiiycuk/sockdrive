//
// Created by caiii on 30.08.2023.
//

#ifndef JS_DOS_SOCKDRIVE_H
#define JS_DOS_SOCKDRIVE_H

#include <stddef.h>
#include <stdint.h>

size_t sockdrive_open(const char* host, uint16_t port);
uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t * buffer);
uint8_t sockdrive_write(size_t handle, uint32_t sector, uint8_t* buffer);
void sockdrive_close(size_t handle);

#endif //JS_DOS_SOCKDRIVE_H
