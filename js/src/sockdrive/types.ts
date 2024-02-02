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
