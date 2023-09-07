import { Ptr, Stats } from "./types";
import { BlockCache, Cache } from "./cache";

declare const Module: {
    HEAPU8: Uint8Array,
    _malloc: (len: number) => Ptr,
    _free: (ptr: Ptr) => void,
    _decode_lz4_block: (compressedSize: number, decodedSize: number, ptr: Ptr) => number,
};

interface Request {
    type: 1 | 2, // read | write
    sector: number,
    buffer: Ptr | Uint8Array,
    resolve: (result: number) => void,
}

export class Drive {

    sectorSize = 512;
    url: string;
    stats: Stats;

    socket: Promise<WebSocket>;
    request: Request | null;

    aheadRange: number;
    aheadSize: number;
    writeBuffer: Uint8Array;

    readBuffer: Uint8Array;
    readAheadBuffer: Ptr;
    readAheadPos = 0;
    readAheadCompressed = 0;
    readStartedAt: number;

    cache: Cache;

    public constructor(url: string, stats: Stats, aheadRange = 255, memoryLimit = 32 * 1024 * 1024) {
        this.url = url;
        this.request = null;
        this.readBuffer = new Uint8Array(1 + 4 + 1);
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.readAheadBuffer = Module._malloc(this.sectorSize * aheadRange);
        this.stats = stats;
        this.aheadRange = aheadRange;
        this.aheadSize = aheadRange * this.sectorSize;
        this.cache = new BlockCache(this.sectorSize, aheadRange, memoryLimit);

        this.reconnect();
    }

    public reconnect(): void {
        this.socket = new Promise<WebSocket>((resolve) => {
            const socket = new WebSocket(this.url);
            socket.binaryType = "arraybuffer";
            const onMessage = this.onMessage.bind(this);
            const onOpen = () => {
                resolve(socket);
            };
            const onError = (e: Event) => {
                console.error("Network error", e, "will reconnect");
                socket.removeEventListener("message", onMessage)
                socket.removeEventListener("open", onOpen);
                socket.removeEventListener("error", onError);
                socket.close();
                setTimeout(this.reconnect.bind(this), 300);
            };
            socket.addEventListener("message", onMessage);
            socket.addEventListener("error", onError);
            socket.addEventListener("open", onOpen);
        });

        if (this.request !== null) {
            this.executeRequest(this.request);
        }
    }

    public read(sector: number, buffer: Ptr, sync: boolean): Promise<number> | number {
        if (this.request !== null && this.request.type === 1) {
            console.error("New read request while old one is still processed");
            return sync ? 3 : Promise.resolve(3);
        }

        const cached = this.cache.read(sector);
        if (cached !== null) {
            if (sync) {
                this.stats.cacheHit++;
            }
            this.request = null;
            Module.HEAPU8.set(cached, buffer);
            return sync ? 0 : Promise.resolve(0);
        } else if (sync) {
            return 255;
        }

        this.stats.cacheMiss++;

        return new Promise<number>((resolve) => {
            this.request = {
                type: 1,
                sector,
                buffer,
                resolve,
            };
            this.executeRequest(this.request);
        });
    }

    public write(sector: number, buffer: Ptr): number {
        this.request = {
            type: 2,
            sector,
            buffer: Module.HEAPU8.slice(buffer, buffer + this.sectorSize),
            resolve: () => { /**/ },
        };
        this.cache.write(sector, this.request.buffer as Uint8Array);
        this.executeRequest(this.request);

        return 0;
    }

    public close() {
        this.socket
            .then((s) => s.close())
            .catch((e) => console.error("Can't close socket", e))
            .finally(() => {
                Module._free(this.readAheadBuffer);
                this.readAheadBuffer = 0;
            });
    }

    private executeRequest(request: Request) {
        this.socket.then((socket) => {
            if (request.type === 1) {
                const { sector } = request;
                const origin = this.cache.getOrigin(sector);
                this.readStartedAt = Date.now();
                this.readAheadPos = 0;
                this.readAheadCompressed = 0;

                this.readBuffer[0] = 1; // read
                this.readBuffer[1] = origin & 0xFF;
                this.readBuffer[2] = (origin >> 8) & 0xFF;
                this.readBuffer[3] = (origin >> 16) & 0xFF;
                this.readBuffer[4] = (origin >> 24) & 0xFF;
                this.readBuffer[5] = this.aheadRange;
                socket.send(this.readBuffer.buffer);
            } else {
                const { sector, buffer, resolve } = request;
                this.stats.write += this.sectorSize;
                this.writeBuffer[0] = 2; // write
                this.writeBuffer[1] = sector & 0xFF;
                this.writeBuffer[2] = (sector >> 8) & 0xFF;
                this.writeBuffer[3] = (sector >> 16) & 0xFF;
                this.writeBuffer[4] = (sector >> 24) & 0xFF;
                // TBD: maybe do not copy and send just slice ???
                this.writeBuffer.set(buffer as Uint8Array, 5);
                socket.send(this.writeBuffer.buffer);
                resolve(0);
            }
        });
    }

    public onMessage(event: { data: ArrayBuffer }) {
        if (this.request === null) {
            console.error("Received message without request");
            this.reconnect();
        } else if (this.request.type === 2) {
            console.error("Received read payload while write request");
            this.reconnect();
        } else if (event.data instanceof ArrayBuffer) {
            let data = new Uint8Array(event.data);
            if (this.readAheadCompressed === 0) {
                this.readAheadCompressed = data[0] + (data[1] << 8) + (data[2] << 16) + (data[3] << 24);
                data = data.slice(4);
            }

            const { sector, buffer, resolve } = this.request;
            const restLength = this.readAheadCompressed - this.readAheadPos;
            if (data.byteLength > restLength || restLength < 0 || restLength > this.aheadSize) {
                console.error("wrong read payload length " + data.byteLength + " instead of " + restLength);
                resolve(3);
            } else {
                Module.HEAPU8.set(data, this.readAheadBuffer + this.readAheadPos);
                this.readAheadPos += data.byteLength;

                if (this.readAheadPos == this.readAheadCompressed) {
                    const decodeResult = Module._decode_lz4_block(this.readAheadCompressed, this.aheadSize, this.readAheadBuffer);
                    if (decodeResult != this.aheadSize) {
                        console.error("wrong decode result " + decodeResult);
                        resolve(4);
                    } else {
                        const origin = this.cache.getOrigin(sector);
                        this.cache.create(origin, Module.HEAPU8.slice(this.readAheadBuffer, this.readAheadBuffer + this.aheadSize));
                        this.stats.cacheUsed = this.cache.memUsed();
                        const offset = sector - origin;
                        Module.HEAPU8.set(
                            Module.HEAPU8.slice(this.readAheadBuffer + offset * this.sectorSize, this.readAheadBuffer + (offset + 1) * this.sectorSize),
                            buffer as number);
                        this.stats.read += this.readAheadCompressed;
                        this.stats.readTotalTime += Date.now() - this.readStartedAt;
                        this.request = null;
                        resolve(0);
                    }
                }
            }
        } else {
            console.error("Received non arraybuffer data");
            this.reconnect();
        }
    }

}