import { FileSystem, FileSystemApi, CreateFileSystemApi, Driver, CreateSockdriveFileSystem } from "../sockdrive-fat";

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

    suite("sockdrive 127.0.0.1:8001");

    test("error on wrong drive name", (done) => {
        createSockdriveFileSystem("ws://127.0.0.1:8001", "system", "not-exists", "", (e) => {
            assert.strictEqual(e.message, "Unable to establish connection");
            done();
        });
    });

    const testSd = (name: string, fn: (fs: FileSystem) => Promise<void>, writeCheck?: number[]) => {
        test(name, async () => {
            const { stats, fs, close } = await createSockdriveFileSystem("ws://127.0.0.1:8001", "system", "test", "");
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
