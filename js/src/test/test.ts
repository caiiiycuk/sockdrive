import { FileSystem, FileSystemApi, CreateFileSystemApi, Driver, CreateSockdriveFileSystem } from "../sockdrive-fat";
import { Drive } from "../sockdrive/drive";
import { EmModule, Stats } from "../sockdrive/types";
import { Cache } from "../sockdrive/cache";

declare const createFileSystem: CreateFileSystemApi;
declare const createSockdriveFileSystem: CreateSockdriveFileSystem;

const baseDir = "/test";
const file = baseDir + "/Simple File.txt";

async function runTests() {
    const assert = chai.assert;
    function createDriver(orig: Uint8Array): Driver {
        const image = orig.slice(0, orig.length);
        const sectorSize = 512;
        return {
            sectorSize,
            numSectors: image.length / 512,
            readSectors: function(sector, dst, cb) {
                dst.set(image.slice(sector * sectorSize, sector * sectorSize + dst.length));
                cb(null, dst);
            },
            writeSectors: function(sector, data, cb) {
                image.set(data, sector * sectorSize);
                cb(null);
            },
        };
    }

    const loadImage = async (url: string) => new Uint8Array(await (await fetch(url)).arrayBuffer());
    const images: { [key: string]: Uint8Array } = {
        fat12: await loadImage("test/fat12.img"),
        fat16: await loadImage("test/fat16.img"),
        fat32: await loadImage("test/fat32.img"),
    };

    const writeBuf = new Uint8Array(4096 * 4);
    for (let i = 0; i < writeBuf.length; ++i) {
        writeBuf[i] = i % 256;
    }

    const testRead = async (fs: FileSystem) => {
        const readBuf = new Uint8Array(writeBuf.length);
        const fd = await fs.fopen(file, "r", 0o666);
        assert.equal((await fs.fstat(fd)).size, writeBuf.length);
        assert.equal(await fs.fread(fd, readBuf, 0, readBuf.length, 0), readBuf.length);
        assert.deepEqual(readBuf, writeBuf);
        await fs.fclose(fd);
    };

    const testOpenWriteStatClose = async (fs: FileSystem) => {
        try {
            await fs.mkdir(baseDir);
        } catch (e) {
            // ignore if exists
        }
        const fd = await fs.fopen(file, "w", 0o666);
        assert.ok(fd);
        assert.equal(await fs.fwrite(fd, writeBuf, 0, writeBuf.length, null), writeBuf.length);
        await fs.fclose(fd);
        await testRead(fs);
    };

    const testHelpers = async (fs: FileSystem) => {
        assert.isFalse(await fs.exists("/not-exists"), "/not-exists not exits");
        assert.isTrue(await fs.exists(baseDir), "baseDir exists");
        assert.isTrue(await fs.exists(file), "file exists");
        assert.isTrue(await fs.isdir(baseDir), "baseDir is dir");
        assert.isFalse(await fs.isdir(file), "file is file");
    };

    for (const name of Object.keys(images)) {
        const driver = () => createDriver(images[name]);
        const fs = () => new Promise<FileSystemApi>((resolve, reject) => {
            const fs = createFileSystem(driver(), {}, (ev, reason) => {
                if (ev === "ready") {
                    resolve(fs);
                } else {
                    console.error(ev, reason);
                    reject(reason);
                }
            });
        });
        suite(name + ".driver");

        test("create driver", () => {
            assert.exists(driver());
        });

        test("create fs", async () => {
            assert.exists(await fs());
        });

        const testFs = (name: string, fn: (fs: FileSystem) => Promise<void>) => {
            test(name, async () => fn(new FileSystem(await fs())));
        };

        testFs("api", async (fs) => {
            for (const method of [
                "mkdir", "readdir",
                // "rename", "unlink", "rmdir",
                "fclose", "fopen", "fwrite", "fread", "fstat",
                "stat", "exists", "isdir",
                // "fsync",
                // "ftruncate", "truncate",
                // "readFile", "writeFile", "appendFile",

                // "chown", "lchown", "fchown",
                // "chmod", "lchmod", "fchmod",
                // "utimes", "futimes",
                // "lstat", "fstat",
                // "link", "symlink", "readlink", "realpath",

                // 'watchfile','unwatchfile','watch'
            ]) {
                assert.ok(method in fs, "fs." + method + " has implementation.");
            };
        });

        testFs("readdir", async (fs) => {
            const files = await fs.readdir("/");
            assert.ok(Array.isArray(files));
            assert.ok(files.length === 0, "image is empty");
        });

        testFs("mkdir", async (fs) => {
            await fs.mkdir(baseDir);
            assert.ok((await fs.readdir(baseDir)).length === 0, "base dir is empty");
            const root = await fs.readdir("/");
            assert.ok(root.length === 1, "root have 1 folder");
            assert.equal("/" + root[0], baseDir);
        });

        testFs("open/write/stat/read/close", testOpenWriteStatClose);

        testFs("exists/isdir", async (fs) => {
            await testOpenWriteStatClose(fs);
            await testHelpers(fs);
        });
    }

    suite("Cache 127.0.0.1:8001");
    test("Cache read/write test", async () => {
        const cache = new Cache("ws://127.0.0.1:8001", true);
        cache.open("system", "test", "");

        await (new Promise<void>((resolve) => setTimeout(resolve, 1000)));
        assert.ok(cache.memUsed() > 0, "cache should contain some data");

        assert.deepEqual(cache.read("system", "test", 0)
            .slice(0, 10), new Uint8Array([51, 192, 142, 208, 188, 0, 124, 251, 80, 7]));

        assert.deepEqual(cache.read("system", "test", 8192)
            .slice(0, 10), new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    });


    suite("Drive 127.0.0.1:8001");

    const testDrive = (name: string,
        fn: (drive: Drive, module: EmModule, stats: Stats) => Promise<void>,
        preload = false) => {
        test(name, async () => {
            const module: EmModule = {
                HEAPU8: new Uint8Array(512),
            };

            const stats: Stats = {
                read: 0,
                write: 0,
                readTotalTime: 0,
                cacheHit: 0,
                cacheMiss: 0,
                cacheUsed: 0,
                io: [],
            };

            const drive = new Drive("ws://127.0.0.1:8001", "system", "test", "", stats, module,
                new Cache("ws://127.0.0.1:8001", preload));
            await new Promise<void>((resolve, reject) => {
                drive.onOpen(() => resolve());
                drive.onError((e) => {
                    reject(e);
                });
            });

            await fn(drive, module, stats);
            await drive.close();
        });
    };

    testDrive("read sector", async (drive, module, stats) => {
        assert.equal(await drive.read(0, 0, false), 0);
        assert.deepEqual(module.HEAPU8.slice(0, 10), new Uint8Array([51, 192, 142, 208, 188, 0, 124, 251, 80, 7]));
        assert.equal(1, stats.cacheMiss, "cache miss (1st read)");
        assert.equal(0, stats.cacheHit, "cache hit (1st read)");

        assert.equal(await drive.read(8192, 0, false), 0);
        assert.deepEqual(module.HEAPU8.slice(0, 10), new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert.equal(2, stats.cacheMiss, "cache miss (2nd read)");
        assert.equal(0, stats.cacheHit, "cache hit (2nd read)");

        assert.equal(await drive.read(8448, 0, false), 0);
        assert.deepEqual(module.HEAPU8.slice(256, 256 + 10), new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]));
        assert.equal(2, stats.cacheMiss, "cache miss (3rd read)");
        assert.equal(1, stats.cacheHit, "cache hit (3rd read)");
    });

    testDrive("read cache", async (drive, module, stats) => {
        assert.equal(await drive.read(0, 0, false), 0);
        assert.equal(0, stats.cacheHit, "cache hit");
        assert.equal(1, stats.cacheMiss, "cache miss");

        for (let i = 0; i < 128; ++i) {
            assert.equal(await drive.read(1 + i, 0, true), 0);
            assert.equal(i + 1, stats.cacheHit, "cache hit");
            assert.equal(1, stats.cacheMiss, "cache miss");
            if (i == 62) {
                assert.deepEqual(module.HEAPU8.slice(0, 10),
                    new Uint8Array([235, 88, 144, 77, 83, 87, 73, 78, 52, 46]));
            }
            if (i == 127) {
                assert.deepEqual(module.HEAPU8.slice(0, 10),
                    new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
            }
        }
    });

    // TODO
    // testDrive("preload sectors", async (drive, module, stats) => {
    //     let _0 = false;
    //     let _8192 = false;
    //     for (const next of preloadQueue) {
    //         _0 = _0 || next === 0;
    //         _8192 = _8192 || next === 8192;
    //     }

    //     await new Promise<void>((resolve) => setTimeout(resolve, 1000));

    //     assert.equal(drive.read(0, 0, true), 0, "sector 0 in cache");
    //     assert.deepEqual(module.HEAPU8.slice(0, 10), new Uint8Array([51, 192, 142, 208, 188, 0, 124, 251, 80, 7]));

    //     assert.equal(await drive.read(8192, 0, true), 0, "sector 8192 in cache");
    //     assert.deepEqual(module.HEAPU8.slice(0, 10), new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));

    //     assert.equal(await drive.read(8448, 0, true), 0, "sector 8448 in cache");
    //     assert.deepEqual(module.HEAPU8.slice(256, 256 + 10), new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]));

    //     assert.equal(3, stats.cacheHit, "cache hit");
    //     assert.equal(0, stats.cacheMiss, "cache miss");
    // }, true);

    testDrive("recovery from socket close", async (drive, module, stats) => {
        const socket = await drive.currentSocket();
        assert.ok(socket != null);
        assert.equal(await drive.read(0, 0, false), 0);
        assert.deepEqual(module.HEAPU8.slice(0, 10), new Uint8Array([51, 192, 142, 208, 188, 0, 124, 251, 80, 7]));

        socket.close();
        assert.equal(await drive.read(8448, 0, false), 0);
        assert.deepEqual(module.HEAPU8.slice(256, 256 + 10), new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]));
    });

    suite("sockdrive 127.0.0.1:8001");

    test("error on wrong drive name", (done) => {
        createSockdriveFileSystem("ws://127.0.0.1:8001", "system", "not-exists", "",
            () => { },
            (e) => {
                assert.strictEqual(e.message, "Err: no such drive");
                done();
            });
    });

    const testSd = (name: string, fn: (fs: FileSystem) => Promise<void>, writeCheck?: number[]) => {
        test(name, async () => {
            const { stats, fs, close } = await createSockdriveFileSystem("ws://127.0.0.1:8001", "system", "test", "",
                (read, write) => {
                    assert.isTrue(read, "read access");
                    assert.isTrue(write, "write access");
                });
            await fn(fs);
            await new Promise<void>((resolve) => {
                setTimeout(resolve, 16);
            });
            await close();
            if (writeCheck) {
                assert.isTrue(writeCheck.findIndex((v) => v === stats.write) >= 0,
                    "write check " + stats.write + " in " + writeCheck);
            }
        });
    };

    testSd("connect", async (fs) => {
        assert.ok(fs);
    });

    testSd("readdir /", async (fs) => {
        assert.ok(await fs.readdir("/"));
    });

    testSd("open/write/stat/read/close", testOpenWriteStatClose);
    testSd("reconnect stat/read/close", testRead);
    testSd("exists/isdir", testHelpers);


    mocha.run();
}

(window as any).runTests = runTests;
