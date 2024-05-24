import { LRUMap } from "./lru";

export interface Cache {
    read(sector: number, originReadMode: boolean): Uint8Array | null;
    write(sector: number, buffer: Uint8Array): boolean;
    create(origin: number, buffer: Uint8Array): void;
    getOrigin(sector: number): number;
    memUsed(): number;
}

export class BlockCache implements Cache {
    aheadRange: number;
    sectorSize: number;
    lru: LRUMap;

    constructor(sectorSize: number, aheadRange: number, memoryLimit: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.lru = new LRUMap(Math.floor(memoryLimit / (aheadRange * sectorSize)));
    }

    public read(sector: number, originReadMode: boolean): Uint8Array | null {
        const origin = this.getOrigin(sector);
        const cached = this.lru.get(origin) as Uint8Array;
        if (cached) {
            if (originReadMode) {
                return cached;
            } else {
                const offset = sector - origin;
                return cached.slice(offset * this.sectorSize, (offset + 1) * this.sectorSize);
            }
        }
        return null;
    }

    public write(sector: number, buffer: Uint8Array) {
        const origin = this.getOrigin(sector);
        const cached = this.lru.get(origin);
        if (cached) {
            cached.set(buffer, (sector - origin) * this.sectorSize);
        }
        return cached !== undefined;
    }

    public create(origin: number, buffer: Uint8Array) {
        this.lru.set(origin, buffer.slice(0));
    }

    public getOrigin(sector: number) {
        return sector - sector % this.aheadRange;
    }

    public memUsed() {
        return this.lru.size * this.aheadRange * this.sectorSize;
    }
}
