import { Ptr, Stats } from "./types";
import { BlockAndWriteCache, BlockCache, Cache, SimpleCache } from "./cache";

declare const Module: { HEAPU8: Uint8Array };

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
    writeBuffer: Uint8Array;

    readBuffer: Uint8Array;
    readAheadBuffer: Uint8Array;
    readAheadPos: 0;
    readStartedAt: number;

    cache: Cache;

    public constructor(url: string, stats: Stats, aheadRange = 255, memoryLimit = 32 * 1024 * 1024) {
        this.url = url;
        this.request = null;
        this.readBuffer = new Uint8Array(1 + 4 + 1);
        this.writeBuffer = new Uint8Array(1 + 4 + this.sectorSize);
        this.readAheadBuffer = new Uint8Array(this.sectorSize * aheadRange),
            this.stats = stats;
        this.aheadRange = aheadRange;
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
            .catch((e) => console.error("Can't close socket", e));

    }

    private executeRequest(request: Request) {
        this.socket.then((socket) => {
            if (request.type === 1) {
                const { sector } = request;
                const origin = this.cache.getOrigin(sector);
                this.readStartedAt = Date.now();
                this.readAheadPos = 0;

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
            const restLength = this.readAheadBuffer.length - this.readAheadPos;
            if (event.data.byteLength > restLength) {
                console.error("wrong read payload length " + event.data.byteLength + " instead of " + restLength);
            } else {
                this.readAheadBuffer.set(new Uint8Array(event.data), this.readAheadPos);
                this.readAheadPos += event.data.byteLength;

                if (this.readAheadPos == this.readAheadBuffer.length) {
                    const { sector, buffer, resolve } = this.request;
                    const origin = this.cache.getOrigin(sector);
                    this.cache.create(origin, this.readAheadBuffer);
                    this.stats.cacheUsed = this.cache.memUsed();
                    const offset = sector - origin;
                    Module.HEAPU8.set(
                        this.readAheadBuffer.slice(offset * this.sectorSize, (offset + 1) * this.sectorSize),
                        buffer as number);
                    this.stats.read += this.sectorSize * this.aheadRange;
                    this.stats.readTotalTime += Date.now() - this.readStartedAt;
                    this.request = null;
                    resolve(0);
                }
            }
        } else {
            console.error("Received non arraybuffer data");
            this.reconnect();
        }
    }

}