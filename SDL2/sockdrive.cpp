#include <cstdint>
#include <cassert>
#include <unordered_map>
#include "LRUCache11.hpp"

#include "../include/sockdrive.h"
#include "../lz4/lz4.h"
#include "SDL_net.h"

namespace {
    int SDLNet_TCP_Recv_All(TCPsocket sock, void *data, int maxlen) {
        auto *start = (uint8_t *) data;
        auto restlen = maxlen;
        while (restlen > 0) {
            auto read = SDLNet_TCP_Recv(sock, start, maxlen);
            start += read;
            restlen -= read;
            // TODO: add sleep
        }
        return maxlen;
    }

    int decode_lz4_block(uint32_t compressedSize, uint32_t decodedSize, char *buffer) {
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

    class BlockCache {
        const uint32_t sectorSize;
        const uint8_t aheadRange;
        lru11::Cache<uint32_t, std::unique_ptr<uint8_t>> lru;

    public:
        BlockCache(uint32_t sectorSize, uint8_t aheadRange, uint32_t memoryLimit) :
                sectorSize(sectorSize), aheadRange(aheadRange), lru(memoryLimit / (aheadRange * sectorSize), 0) {
        }

        uint8_t *read(uint32_t sector) {
            const auto origin = getOrigin(sector);
            if (lru.contains(origin)) {
                return lru.getRef(origin).get() + (sector - origin) * sectorSize;
            }
            return nullptr;
        }

        bool write(uint32_t sector, uint8_t *buffer) {
            const auto origin = getOrigin(sector);
            if (lru.contains(origin)) {
                memcpy(lru.getRef(origin).get() + (sector - origin) * sectorSize, buffer, sectorSize);
                return true;
            }
            return false;
        }

        void create(uint32_t origin, uint8_t *buffer) {
            if (lru.contains(origin)) {
                memcpy(lru.getRef(origin).get(), buffer, sectorSize * aheadRange);
            } else {
                auto copy = new uint8_t[sectorSize * aheadRange];
                memcpy(copy, buffer, sectorSize * aheadRange);
                lru.insert(origin, copy);
            }
        }

        uint32_t getOrigin(uint32_t sector) const {
            return sector - sector % aheadRange;
        }

    };

    class Drive {
        const uint32_t sectorSize = 512;

        TCPsocket socket;
        const uint8_t aheadRange;
        uint8_t *readAheadBuffer;
        uint32_t aheadSize;

        BlockCache cache;

    public:
        Drive(TCPsocket socket, uint8_t aheadRange = 255, uint32_t memoryLimit = 32 * 1024 * 1024) :
                socket(socket), aheadRange(aheadRange), cache(sectorSize, aheadRange, memoryLimit) {

            aheadSize = sectorSize * aheadRange;
            readAheadBuffer = new uint8_t[aheadSize];
        }

        ~Drive() {
            SDLNet_TCP_Close(socket);
            delete[] readAheadBuffer;
        }

        uint8_t read(uint32_t sector, uint8_t *buffer) {
            auto cached = cache.read(sector);
            if (cached) {
                memcpy(buffer, cached, sectorSize);
                return 0;
            }

            static const uint8_t readCommand = 1;
            if (SDLNet_TCP_Send(socket, &readCommand, sizeof(uint8_t)) != sizeof(uint8_t)) {
                return 2;
            }
            auto origin = cache.getOrigin(sector);
            if (SDLNet_TCP_Send(socket, &origin, sizeof(uint32_t)) != sizeof(uint32_t)) {
                return 3;
            }
            if (SDLNet_TCP_Send(socket, &aheadRange, sizeof(uint8_t)) != sizeof(uint8_t)) {
                return 4;
            }
            if (SDLNet_TCP_Recv_All(socket, readAheadBuffer, sizeof(uint32_t)) != sizeof(uint32_t)) {
                return 5;
            }
            uint32_t compressedSize = readAheadBuffer[0] + (readAheadBuffer[1] << 8) + (readAheadBuffer[2] << 16) + (readAheadBuffer[3] << 24);
            if (SDLNet_TCP_Recv_All(socket, readAheadBuffer, compressedSize) != compressedSize) {
                return 6;
            }

            int decodeResult = decode_lz4_block(compressedSize, aheadSize, (char *) readAheadBuffer);
            if (decodeResult != aheadSize) {
                return decodeResult;
            }

            cache.create(origin, readAheadBuffer);
            memcpy(buffer, readAheadBuffer + (sector - origin) * sectorSize, sectorSize);

            return 0;
        }

        uint8_t write(uint32_t sector, uint8_t *buffer) {
            cache.write(sector, buffer);
            static const uint8_t writeCommand = 2;
            if (SDLNet_TCP_Send(socket, &writeCommand, sizeof(uint8_t)) != sizeof(uint8_t)) {
                return 2;
            }
            if (SDLNet_TCP_Send(socket, &sector, sizeof(uint32_t)) != sizeof(uint32_t)) {
                return 3;
            }
            if (SDLNet_TCP_Send(socket, buffer, sectorSize) != sectorSize) {
                return 4;
            }
            return 0;
        }
    };

}

size_t sockdrive_open(const char* url, const char* owner, const char* name, const char* token) {
    IPaddress address;
    if (SDLNet_ResolveHost(&address, url, 8001) == 0) {
        return reinterpret_cast<size_t>(new Drive(SDLNet_TCP_Open(&address)));
    }

    return 0;
}

uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t *buffer) {
    if (!handle) {
        return 1;
    }

    return reinterpret_cast<Drive*>(handle)->read(sector, buffer);
}

uint8_t sockdrive_write(size_t handle, uint32_t sector, uint8_t *buffer) {
    if (!handle) {
        return 1;
    }

    return reinterpret_cast<Drive*>(handle)->write(sector, buffer);
}

void sockdrive_close(size_t handle) {
    if (handle) {
        delete reinterpret_cast<Drive*>(handle);
    }
}

uint32_t sockdrive_size(size_t handle) {
    return 2097152;
}
uint32_t sockdrive_heads(size_t handle) {
    return 128;
}
uint32_t sockdrive_sectors(size_t handle) {
    return 63;
}
uint32_t sockdrive_cylinders(size_t handle) {
    return 520;
}
uint32_t sockdrive_sector_size(size_t handle) {
    return 512;
}
