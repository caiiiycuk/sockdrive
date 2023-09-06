import { read } from "fs";
import { LRUMap } from "./lru";

export class Cache {

    aheadRange: number;
    sectorSize: number;
    lru: LRUMap;

    constructor(aheadRange: number, memoryLimit: number, sectorSize: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.lru = new LRUMap(Math.floor(memoryLimit / (aheadRange * sectorSize)));
    }

    public read(sector: number): Uint8Array | null {
        const cached = this.lru.get(this.getOrigin(sector));
        if (cached) {
            const offset = this.getOffset(sector);
            return cached.slice(offset * this.sectorSize, (offset + 1) * this.sectorSize);
        }
        return null;
    }

    public write(sector: number, buffer: Uint8Array) {
        const cached = this.lru.get(this.getOrigin(sector));
        if (cached) {
            const offset = this.getOffset(sector);
            cached.set(buffer, offset * this.sectorSize);
        }
    }

    public create(origin: number, buffer: Uint8Array) {
        if (!this.lru.get(origin)) {
            this.lru.set(origin, buffer);
        }
    }

    public getOrigin(sector: number) {
        return Math.floor(sector / this.aheadRange);
    }

    getOffset(sector: number) {
        return sector % this.aheadRange;
    }

}