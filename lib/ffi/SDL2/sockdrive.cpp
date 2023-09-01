#include "../sockdrive.h"
#include "SDL_net.h"

#define getSocket(handle) reinterpret_cast<TCPsocket>(handle)
constexpr uint16_t sectorSize = 512;

size_t sockdrive_open(const char* host, uint16_t port) {
    IPaddress address;
    if (SDLNet_ResolveHost(&address, host, port) == 0) {
        return reinterpret_cast<size_t>(SDLNet_TCP_Open(&address));
    }

    return 0;
}

uint8_t sockdrive_read(size_t handle, uint32_t sector, uint8_t *buffer) {
    if (!handle) {
        return 2;
    }

    TCPsocket socket = getSocket(handle);
    static const uint8_t readCommand = 1;
    if (SDLNet_TCP_Send(socket, &readCommand, sizeof(uint8_t)) != sizeof(uint8_t)) {
        return 1;
    }
    if (SDLNet_TCP_Send(socket, &sector, sizeof(uint32_t)) != sizeof(uint32_t)) {
        return 1;
    }
    if (SDLNet_TCP_Recv(socket, buffer, sectorSize) != sectorSize) {
        return 1;
    }
    return 0;
}

uint8_t sockdrive_write(size_t handle, uint32_t sector, uint8_t* buffer) {
    if (!handle) {
        return 2;
    }

    TCPsocket socket = getSocket(handle);
    static const uint8_t writeCommand = 2;
    if (SDLNet_TCP_Send(socket, &writeCommand, sizeof(uint8_t)) != sizeof(uint8_t)) {
        return 1;
    }
    if (SDLNet_TCP_Send(socket, &sector, sizeof(uint32_t)) != sizeof(uint32_t)) {
        return 1;
    }
    if (SDLNet_TCP_Send(socket, buffer, sectorSize) != sectorSize) {
        return 1;
    }
    return 0;
}

void sockdrive_close(size_t handle) {
    if (handle) {
        SDLNet_TCP_Close(getSocket(handle));
    }
}
