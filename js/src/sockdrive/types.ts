export type Handle = number;
export type Ptr = number;
export interface Stats {
    read: number,
    write: number,
    readTotalTime: number,
    cacheHit: number,
    cacheMiss: number,
    cacheUsed: number,
    io: {
        read: number,
        write: number,
    }[],
};

export interface EmModule {
    HEAPU8: Uint8Array,
};
