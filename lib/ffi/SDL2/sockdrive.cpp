#include "../sockdrive.h"
#include "SDL_net.h"
#include <unordered_map>

#define getSocket(handle) reinterpret_cast<TCPsocket>(handle)

// TODO replace with memory-limited cache
// TODO make cache drive dependent
// TODO fix memory leaking in cache! (each time it leaks)
std::unordered_map<uint32_t, uint8_t*> sectorCache;

constexpr uint16_t sectorSize = 512;
constexpr uint8_t aheadRead = 128; // 64kb

int SDLNet_TCP_Recv_All(TCPsocket sock, void *data, int maxlen) {
   auto *start = (uint8_t*) data;
   auto restlen = maxlen;
   while (restlen > 0) {
       auto readed = SDLNet_TCP_Recv(sock, start, maxlen);
       start += readed;
       restlen -= readed;
       // TODO: add sleep
   }
   return maxlen;
}

size_t sockdrive_open(const char* host, uint16_t port) {
    IPaddress address;
    if (SDLNet_ResolveHost(&address, host, port) == 0) {
        return reinterpret_cast<size_t>(SDLNet_TCP_Open(&address));
    }

    return 0;
}

uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t *buffer) {
    if (!handle) {
        return 1;
    }

    auto it = sectorCache.find(sector);
    if (it != sectorCache.end()) {
        memcpy(buffer, it->second, sectorSize);
        return 0;
    }


    TCPsocket socket = getSocket(handle);
    static const uint8_t readCommand = 1;
    static const uint8_t ahead = aheadRead;
    static uint8_t *aheadBuffer = new uint8_t[sectorSize * aheadRead];
    if (SDLNet_TCP_Send(socket, &readCommand, sizeof(uint8_t)) != sizeof(uint8_t)) {
        return 2;
    }
    if (SDLNet_TCP_Send(socket, &sector, sizeof(uint32_t)) != sizeof(uint32_t)) {
        return 3;
    }
    if (SDLNet_TCP_Send(socket, &ahead, sizeof(uint8_t)) != sizeof(uint8_t)) {
        return 4;
    }
    ;
    if (SDLNet_TCP_Recv_All(socket, aheadBuffer, sectorSize * aheadRead) != sectorSize * aheadRead) {
        return 5;
    }

    memcpy(buffer, aheadBuffer, sectorSize);
    for (int i = 0; i < aheadRead; ++i) {
        if (sectorCache.find(sector + i) == sectorCache.end()) {
            uint8_t *copy = new uint8_t[sectorSize];
            memcpy(copy, aheadBuffer + i * sectorSize, sectorSize);
            sectorCache.insert(std::make_pair<>(sector + i, copy));
        }
    }
    return 0;
}

uint8_t sockdrive_write(size_t handle, uint32_t sector, uint8_t* buffer) {
    if (!handle) {
        return 1;
    }

    uint8_t *copy = new uint8_t[sectorSize];
    memcpy(copy, buffer, sectorSize);
    sectorCache[sector] = copy;

    TCPsocket socket = getSocket(handle);
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

void sockdrive_close(size_t handle) {
    if (handle) {
        SDLNet_TCP_Close(getSocket(handle));
    }
}
