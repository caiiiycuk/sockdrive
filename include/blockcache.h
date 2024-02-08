//
// Created by caiii on 08.02.2024.
//

#ifndef SOCKDRIVE_BLOCKCACHE_H
#define SOCKDRIVE_BLOCKCACHE_H

#include <unistd.h>
#include <cstring>
#include "./LRUCache11.hpp"

class BlockCache {
    const uint32_t sectorSize;
    const uint8_t aheadRange;
    lru11::Cache<uint32_t, std::unique_ptr<std::vector<uint8_t>>> lru;

public:
    BlockCache(uint32_t sectorSize, uint8_t aheadRange, uint32_t memoryLimit) :
            sectorSize(sectorSize), aheadRange(aheadRange), lru(memoryLimit / (aheadRange * sectorSize), 0) {
    }

    uint8_t *read(uint32_t sector) {
        const auto origin = getOrigin(sector);
        if (lru.contains(origin)) {
            return lru.getRef(origin)->data() + (sector - origin) * sectorSize;
        }
        return nullptr;
    }

    bool write(uint32_t sector, uint8_t *buffer) {
        const auto origin = getOrigin(sector);
        if (lru.contains(origin)) {
            memcpy(lru.getRef(origin)->data() + (sector - origin) * sectorSize, buffer, sectorSize);
            return true;
        }
        return false;
    }

    void create(uint32_t origin, uint8_t *buffer) {
        if (lru.contains(origin)) {
            memcpy(lru.getRef(origin)->data(), buffer, sectorSize * aheadRange);
        } else {
            auto copy = new std::vector<uint8_t >(sectorSize * aheadRange);
            memcpy(copy->data(), buffer, sectorSize * aheadRange);
            lru.insert(origin, copy);
        }
    }

    uint32_t getOrigin(uint32_t sector) const {
        return sector - sector % aheadRange;
    }

};

#endif //SOCKDRIVE_CACHE_H
