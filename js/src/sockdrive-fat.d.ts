interface Stats {
    read: number,
    write: number,
    readTotalTime: number,
    cacheHit: number,
    cacheMiss: number,
    cacheUsed: number,
}
interface StatsBase<T> {
    isFile(): boolean;
    isDirectory(): boolean;
    isBlockDevice(): boolean;
    isCharacterDevice(): boolean;
    isSymbolicLink(): boolean;
    isFIFO(): boolean;
    isSocket(): boolean;
    dev: T;
    ino: T;
    mode: T;
    nlink: T;
    uid: T;
    gid: T;
    rdev: T;
    size: T;
    blksize: T;
    blocks: T;
    atimeMs: T;
    mtimeMs: T;
    ctimeMs: T;
    birthtimeMs: T;
    atime: Date;
    mtime: Date;
    ctime: Date;
    birthtime: Date;
}
interface Stats extends StatsBase<number> { }
type FileDescriptor = number;
export interface SockdriveInstance {
    stats: Stats,
    fs: {
        mkdir: (path: string) => Promise<void>,
        readdir: (path: string) => Promise<string[]>,
        fopen: (path: string, flags: string | number, mode: number) => Promise<FileDescriptor>,
        fread: (fd: FileDescriptor, buf: Uint8Array, offset: number,
            length: number, pos: number | null) => Promise<number>,
        fwrite: (fd: FileDescriptor, buf: Uint8Array, offset: number,
            length: number, pos: number | null) => Promise<number>,
        fstat: (fd: FileDescriptor) => Promise<Stats>,
        fclose: (fd: FileDescriptor) => Promise<void>,
        stat: (path: string) => Promise<Stats>,
        isdir: (path: string) => Promise<boolean>,
        exists: (path: string) => Promise<boolean>,
    }
    close: () => void,
}

export type CreateSockdriveFileSystem = (
    endpoint: string,
    ownerId: string,
    driveId: string,
    token: string,
    onOpen: (read: boolean, write: boolean) => void,
    onError: (e: Error) => void
) => Promise<SockdriveInstance>;
