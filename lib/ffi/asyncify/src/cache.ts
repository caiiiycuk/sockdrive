import { LRUMap } from "./lru";

export class Cache {

    aheadRange: number;
    sectorSize: number;
    lru: LRUMap;

    constructor(sectorSize: number, aheadRange: number, memoryLimit: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.lru = new LRUMap(Math.floor(memoryLimit / (aheadRange * sectorSize)));
    }

    public read(sector: number): Uint8Array | null {
        const origin = this.getOrigin(sector);
        const cached = this.lru.get(origin) as Uint8Array;
        if (cached) {
            const offset = sector - origin;
            return cached.slice(offset * this.sectorSize, (offset + 1) * this.sectorSize);
        }
        return null;
    }

    public write(sector: number, buffer: Uint8Array) {
        const origin = this.getOrigin(sector);
        const cached = this.lru.get(origin);
        if (cached) {
            cached.set(buffer, (sector - origin) * this.sectorSize);
        }
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