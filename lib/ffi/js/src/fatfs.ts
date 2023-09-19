const toBuffer = require("typedarray-to-buffer");

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
                    console.log(err);
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
    open(path: string, flags: string | number, mode: number) {
        return this.promisify<FileDescriptor>(this.fs.open, path, flags, mode);
    }
    read(fd: FileDescriptor, buf: Uint8Array, offset: number,
        length: number, pos: number | null) {
        return this.promisify<number>(this.fs.read, fd, toBuffer(buf),
            offset, length, pos);
    }
    write(fd: FileDescriptor, buf: Uint8Array, offset: number,
        length: number, pos: number | null) {
        return this.promisify<number>(this.fs.write, fd, toBuffer(buf),
            offset, length, pos);
    }
    fstat(fd: FileDescriptor) {
        return this.promisify<Stats>(this.fs.fstat, fd);
    }
    exists(fd: FileDescriptor) {
        return this.fstat(fd).then(() => true).catch(() => false);
    }
    close(fd: FileDescriptor) {
        return this.promisify<void>(this.fs.close, fd);
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
