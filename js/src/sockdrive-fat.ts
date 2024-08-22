import { createFileSystem } from "./fatfs";

const toBuffer = require("typedarray-to-buffer");
import { Drive } from "./sockdrive/drive";
import { EmModule, Stats as SockdriveStats } from "./sockdrive/types";

export interface Driver {
    sectorSize: number,
    numSectors: number,
    readSectors: (sector: number, dst: Uint8Array, cb: (error: Error | null, dst: Uint8Array) => void) => void,
    writeSectors: (sector: number, src: Uint8Array, cb: (error: Error | null) => void) => void,
}

type Callback = (error: Error | null) => void;
type CallbackT<T> = (error: Error | null, t: T) => void;

export type FileDescriptor = number;

export interface FileSystemApi {
    mkdir(path: string, cb: Callback): void;
    readdir(path: string, cb: CallbackT<string[]>): void;
    open(path: string, flags: string | number, mode: number, cb: CallbackT<FileDescriptor>): void;
    read(fd: FileDescriptor, buf: Buffer, offset: number,
        length: number, pos: number | null, cb: CallbackT<number>): void;
    write(fd: FileDescriptor, buf: Buffer, offset: number,
        length: number, pos: number | null, cb: CallbackT<number>): void;
    fstat(fd: FileDescriptor, cb: CallbackT<Stats>): void;
    close(fd: FileDescriptor, cb: Callback): void;
}

export type CreateFileSystemApi = (driver: Driver, opts: {
    ro?: boolean,
    noatime?: boolean,
    modmode?: number,
    umask?: number,
    uid?: number,
    gid?: number,
}, event: (event: "ready" | "error", reason: Error | null) => void) => FileSystemApi;
export type CreateSockdriveFileSystem = typeof createSockdriveFileSystem;


export class FileSystem {
    fs: FileSystemApi;
    constructor(fs: FileSystemApi) {
        this.fs = fs;
    }

    private promisify<T>(fn: any, ...args: any[]): Promise<T> {
        return new Promise<T>((resolve, reject) => {
            fn.call(this.fs, ...args, (err: Error | null, res: T) => {
                if (!err) {
                    resolve(res);
                } else {
                    reject(err);
                }
            });
        });
    }

    mkdir(path: string) {
        return this.promisify<void>(this.fs.mkdir, path);
    }
    readdir(path: string) {
        return this.promisify<string[]>(this.fs.readdir, path);
    }
    fopen(path: string, flags: string | number, mode: number) {
        return this.promisify<FileDescriptor>(this.fs.open, path, flags, mode);
    }
    fread(fd: FileDescriptor, buf: Uint8Array, offset: number,
        length: number, pos: number | null) {
        return this.promisify<number>(this.fs.read, fd, toBuffer(buf),
            offset, length, pos);
    }
    fwrite(fd: FileDescriptor, buf: Uint8Array, offset: number,
        length: number, pos: number | null) {
        return this.promisify<number>(this.fs.write, fd, toBuffer(buf),
            offset, length, pos);
    }
    fstat(fd: FileDescriptor) {
        return this.promisify<Stats>(this.fs.fstat, fd);
    }
    fclose(fd: FileDescriptor) {
        return this.promisify<void>(this.fs.close, fd);
    }

    // helpers
    async stat(path: string) {
        const fd: number | null = await (this.fopen(path, "\\r", 0o666)
            .catch((e: Error & { code: string }) => {
                if (e.code === "NOENT") {
                    return null;
                }

                throw e;
            }));
        if (fd === null) {
            return null;
        }
        const stat = await this.fstat(fd);
        await this.fclose(fd);
        return stat;
    }

    async isdir(path: string) {
        const stat = await this.stat(path);
        return stat !== null ? stat.isDirectory() : false;
    }

    async exists(path: string) {
        return (await this.stat(path)) !== null;
    }
}

export interface StatsBase<T> {
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

export interface Stats extends StatsBase<number> { }

export async function createSockdriveFileSystem(endpoint: string,
                                                ownerId: string,
                                                driveId: string,
                                                token: string,
                                                onOpen: (read: boolean, write: boolean) => void = () => { },
                                                onError: (e: Error) => void = () => { }) {
    const stats: SockdriveStats = {
        read: 0,
        write: 0,
        readTotalTime: 0,
        cacheHit: 0,
        cacheMiss: 0,
        cacheUsed: 0,
        io: [],
    };

    const sectorSize = 512;
    const module: EmModule = {
        HEAPU8: new Uint8Array(sectorSize),
    };

    let drive: Drive | null = null;
    const fs = await new Promise<FileSystemApi>((resolve, reject) => {
        drive = new Drive(endpoint, ownerId, driveId, token, stats, module);
        drive.onOpen((read, write, imageSize) => {
            // TODO: fatfs should respect boot record section
            const MBR_OFFSET = 63;
            const driver: Driver = {
                sectorSize,
                numSectors: imageSize / sectorSize,
                readSectors: function(start, dst, cb) {
                    (async () => {
                        start += MBR_OFFSET;
                        // TODO we can avoid copying in dst.set
                        for (let i = 0; i < dst.length / sectorSize; ++i) {
                            if (drive.read(start + i, 0, true) !== 0) {
                                const readCode = await drive.read(start + i, 0, false);
                                if (readCode !== 0) {
                                    cb(new Error("Read error, code: " + readCode), dst);
                                    return;
                                }
                            }
                            dst.set(module.HEAPU8.slice(0, sectorSize), i * sectorSize);
                        }
                    })()
                        .then(() => cb(null, dst))
                        .catch((e) => cb(e, dst));
                },
                writeSectors: function(start, data, cb) {
                    start += MBR_OFFSET;
                    for (let i = 0; i < data.length / sectorSize; ++i) {
                        module.HEAPU8.set(data.slice(i * sectorSize, (i + 1) * sectorSize), 0);
                        const writeCode = drive.write(start + i, 0);
                        if (writeCode !== 0) {
                            cb(new Error("Write error, code: " + 0));
                            return;
                        }
                    }
                    cb(null);
                },
            };
            const fs = (createFileSystem as any as CreateFileSystemApi)(driver, {}, (ev, reason) => {
                if (ev === "ready") {
                    resolve(fs);
                } else {
                    console.error(ev, reason);
                    reject(reason);
                }
            });
            onOpen(read, write);
        });
        drive.onError(onError);
    });

    return {
        stats,
        fs: new FileSystem(fs),
        close: () => drive?.close(),
    };
}


(window as any).createSockdriveFileSystem = createSockdriveFileSystem;
(window as any).createFileSystem = createFileSystem;
