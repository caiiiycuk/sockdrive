export type Handle = number;
export type Ptr = number;
export interface Stats {
    read: number,
    write: number,
    readTotalTime: number,
    cacheHit: number,
    cacheMiss: number,
    cacheUsed: number,
};

export interface EmModule {
    HEAPU8: Uint8Array,
    _malloc: (len: number) => Ptr,
    _free: (ptr: Ptr) => void,
};
