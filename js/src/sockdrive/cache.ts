import { decodeFrame, Frame } from "./decode";
import { LRUMap } from "./lru";
import { MAX_FRAME_SIZE } from "./decode";

const MEMORY_LIMIT = 64 * 1024 * 1024;

type ProgressFn = (owner: string, drive: string, rest: number, total: number) => void;
type PayloadFn = (owner: string, drive: string, sectorSize: number,
    aheadRange: number, sectors: number[], row: Uint8Array) => void;

class BlockCache {
    aheadRange: number;
    sectorSize: number;
    aheadSize: number;
    lru: LRUMap;

    constructor(sectorSize: number, aheadRange: number, memoryLimit: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.aheadSize = aheadRange * sectorSize;
        this.lru = new LRUMap(Math.floor(memoryLimit / (aheadRange * sectorSize)));
    }

    contains(origin: number): boolean {
        return !!this.lru.get(origin);
    }

    public read(sector: number): Uint8Array | null {
        const origin = this.getOrigin(sector);
        const cached = this.lru.get(origin) as Uint8Array;
        if (cached) {
            const offset = sector - origin;
            return offset === 0 ? cached : cached.slice(offset * this.sectorSize, (offset + 1) * this.sectorSize);
        }
        return null;
    }

    public create(origin: number, buffer: Uint8Array, from: number) {
        this.lru.set(origin, buffer.slice(from, from + this.aheadSize));
    }

    public getOrigin(sector: number) {
        return sector - sector % this.aheadRange;
    }

    public memUsed() {
        return this.lru.size * this.aheadSize;
    }
}

class WsCache {
    cache: BlockCache | null = null;
    writeCache: {[sector: number]: Uint8Array} = {};
    socket: WebSocket | null = null;
    connect: () => Promise<void>;

    constructor(url: string, owner: string, drive: string, token: string, preload: boolean,
        progress: ProgressFn, payload: PayloadFn) {
        this.connect = () => {
            return new Promise<void>((resolve) => {
                const socket = new WebSocket(url);
                this.socket = socket;
                socket.addEventListener("close", () => {
                    resolve();
                });
                socket.binaryType = "arraybuffer";
                socket.addEventListener("error", () => {
                    console.error("Can't connect to", url, owner, drive);
                    socket.close();
                });
                socket.addEventListener("open", () => {
                    socket.addEventListener("message", (event: { data: ArrayBuffer }) => {
                        const data = new Uint32Array(event.data);
                        if (data?.length !== 3) {
                            console.error("Wrong cache format");
                        } else {
                            const sectorSize = data[0];
                            const aheadRange = data[1];
                            this.cache = new BlockCache(sectorSize, aheadRange, MEMORY_LIMIT);
                            if (!preload) {
                                socket.close();
                                return;
                            }

                            let count = data[2];
                            if (count === 0) {
                                socket.close();
                            } else {
                                const aheadSize = sectorSize * aheadRange;
                                const maxSectorsCount = Math.floor(MAX_FRAME_SIZE / aheadSize);
                                const frame: Frame = {
                                    payload: new Uint8Array(MAX_FRAME_SIZE),
                                    sectorsRow: new Uint8Array(MAX_FRAME_SIZE),
                                    aheadSize: aheadRange * sectorSize,
                                    sectors: [],
                                    payloadSize: 0,
                                    payloadPos: 0,
                                };
                                const total = count * aheadSize;
                                let rest = total;
                                progress(owner, drive, rest, total);
                                socket.addEventListener("message", (event: { data: ArrayBuffer }) => {
                                    if (event.data instanceof ArrayBuffer) {
                                        const data = new Uint8Array(event.data);
                                        if (frame.sectors.length === 0) {
                                            const sectorsCount = data[1] + (data[1 + 1] << 8) +
                                                (data[1 + 2] << 16) + (data[1 + 3] << 24);
                                            if (sectorsCount > maxSectorsCount) {
                                                console.error("Can't load more than", maxSectorsCount,
                                                    "at once (requested" + sectorsCount + ")");
                                                socket.close();
                                                return;
                                            }
                                            for (let i = 0; i < sectorsCount; ++i) {
                                                const offset = 5 + i * 4;
                                                frame.sectors.push(data[offset] + (data[offset + 1] << 8) +
                                                    (data[offset + 2] << 16) + (data[offset + 3] << 24));
                                            }
                                        } else {
                                            if (decodeFrame(event, frame)) {
                                                if (frame.error) {
                                                    console.error(frame.error);
                                                    socket.close();
                                                } else {
                                                    payload(owner, drive, sectorSize, aheadRange,
                                                        frame.sectors, frame.sectorsRow);
                                                    rest -= frame.sectors.length * aheadSize;
                                                    count -= frame.sectors.length;
                                                    progress(owner, drive, rest, total);
                                                    for (let i = 0; i < frame.sectors.length; ++i) {
                                                        if (!this.cache?.contains(frame.sectors[i])) {
                                                            this.cache?.create(frame.sectors[i],
                                                                frame.sectorsRow, i * frame.aheadSize);
                                                        }
                                                    }
                                                    frame.sectors = [];
                                                    frame.payloadSize = 0;
                                                    frame.payloadPos = 0;

                                                    if (count === 0) {
                                                        socket.close();
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        console.error("Unknown event", event);
                                        socket.close();
                                    }
                                });
                            }
                        }
                    }, { once: true });
                    socket.send(owner + "&" + drive + "&" + token);
                });
            });
        };
    }

    public read(sector: number): Uint8Array | null {
        return this.writeCache[sector] ?? this.cache?.read(sector) ?? null;
    }

    public write(sector: number, buffer: Uint8Array): void {
        this.writeCache[sector] = buffer;
    }

    public create(origin: number, buffer: Uint8Array, from: number) {
        this.cache?.create(origin, buffer, from);
    }

    public getOrigin(sector: number) {
        return this.cache?.getOrigin(sector) ?? 0;
    }

    public memUsed() {
        return this.cache?.memUsed() ?? 0;
    }

    public close() {
        this.socket?.close();
    }
}

export class Cache {
    private url: string;
    private preload: boolean;
    private impl: { [key: string]: WsCache } = {};
    private progress: ProgressFn = () => {
    };
    private payload: PayloadFn = () => {
    };
    private key(owner: string, drive: string) {
        return owner + "/" + drive;
    }
    private connectQueue: (() => Promise<void>)[] = [];

    constructor(url: string, preload: boolean) {
        this.url = url;
        this.preload = preload;
    }

    open(owner: string, drive: string, token: string) {
        const key = this.key(owner, drive);
        if (this.impl[key] === undefined) {
            const cache = new WsCache(this.url + "/cache", owner, drive, token,
                this.preload, this.progress, this.payload);
            this.impl[key] = cache;
            this.connectQueue.push(cache.connect);
            if (this.connectQueue.length == 1) {
                this.runNextCache();
            }
        }
    }

    runNextCache() {
        if (this.connectQueue.length > 0) {
            const next = this.connectQueue[0];
            next().then(() => {
                if (next === this.connectQueue[0]) {
                    this.connectQueue.splice(0, 1);
                    this.runNextCache();
                } else {
                    console.error("wrong cache order!");
                }
            });
        }
    }

    read(owner: string, drive: string, sector: number): Uint8Array | null {
        const key = this.key(owner, drive);
        if (this.impl[key]) {
            return this.impl[key].read(sector);
        } else {
            console.error("Cache for drive", key, "not opened!");
            return null;
        }
    }

    update(owner: string, drive: string, origin: number, buffer: Uint8Array, from: number): void {
        const key = this.key(owner, drive);
        if (this.impl[key]) {
            const expected = this.impl[key].getOrigin(origin);
            if (expected && expected !== origin) {
                throw new Error("Origin mistamtch for " + key);
            }
            this.impl[key].create(origin, buffer, from);
        } else {
            console.error("Cache for drive", key, "not opened!");
        }
    }

    write(owner: string, drive: string, sector: number, buffer: Uint8Array): void {
        const key = this.key(owner, drive);
        if (this.impl[key]) {
            this.impl[key].write(sector, buffer);
        } else {
            console.error("Cache for drive", key, "not opened!");
        }
    }

    memUsed(): number {
        let total = 0;
        for (const key of Object.keys(this.impl)) {
            total += this.impl[key].memUsed();
        }
        return total;
    }

    onProgress(fn: ProgressFn) {
        this.progress = fn;
    }

    onPayload(fn: PayloadFn) {
        this.payload = fn;
    }

    close() {
        for (const key of Object.keys(this.impl)) {
            this.impl[key].close();
        }
    }
}
