import { LRUMap } from "./lru";

export interface Cache {
    read(sector: number): Uint8Array | null;
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

export class SimpleCache implements Cache {

    aheadRange: number;
    sectorSize: number;
    lru: LRUMap;

    constructor(sectorSize: number, aheadRange: number, memoryLimit: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.lru = new LRUMap(Math.floor(memoryLimit / sectorSize));
    }

    public read(sector: number): Uint8Array | null {
        const cached = this.lru.get(sector) as Uint8Array;
        if (cached) {
            return cached;
        }
        return null;
    }

    public write(sector: number, buffer: Uint8Array) {
        this.lru.set(sector, buffer);
        return true;
    }

    public create(sector: number, buffer: Uint8Array) {
        for (let i = 0; i < this.aheadRange; ++i) {
            this.lru.set(sector + i, buffer.slice(i * this.sectorSize, (i + 1) * this.sectorSize));
        }
    }
    
    public getOrigin(sector: number) {
        return sector;
    }

    public memUsed() {
        return this.lru.size * this.sectorSize;
    }
}

export class BlockAndWriteCache implements Cache {

    aheadRange: number;
    sectorSize: number;
    blockCache: BlockCache;
    writeCache: SimpleCache;

    constructor(sectorSize: number, aheadRange: number, memoryLimit: number) {
        this.aheadRange = aheadRange;
        this.sectorSize = sectorSize;
        this.blockCache = new BlockCache(sectorSize, aheadRange, memoryLimit);
        this.writeCache = new SimpleCache(sectorSize, aheadRange, Math.floor(memoryLimit / 10));
    }

    read(sector: number) {
        return this.blockCache.read(sector) || this.writeCache.read(sector);
    }

    write(sector: number, buffer: Uint8Array) {
        if (!this.blockCache.write(sector, buffer)) {
            this.writeCache.write(sector, buffer);
        }
        return true;
    }

    create(origin: number, buffer: Uint8Array): void {
        this.blockCache.create(origin, buffer);
        for (let i = 0; i < this.aheadRange; ++i) {
            if (this.writeCache.read(origin + i) != null) {
                this.writeCache.write(origin + i, buffer.slice(i * this.sectorSize, (i + 1) * this.sectorSize));
            }
        }
    }
    
    getOrigin(sector: number): number {
        return this.blockCache.getOrigin(sector);
    }

    memUsed(): number {
        return this.blockCache.memUsed() + this.writeCache.memUsed();
    }
}