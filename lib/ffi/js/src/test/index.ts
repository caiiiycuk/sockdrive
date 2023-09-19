import { FileSystem, FileSystemApi, CreateFileSystemApi, Driver } from "../fatfs";

declare const createFileSystem: CreateFileSystemApi;

const baseDir = "/fat_test-" + Math.random().toFixed(20).slice(2);
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
                "close", "open", "write", "read", "fstat",
                // "fsync",
                // "ftruncate", "truncate",
                // "readFile", "writeFile", "appendFile",

                // "chown", "lchown", "fchown",
                // "chmod", "lchmod", "fchmod",
                // "utimes", "futimes",
                // "stat", "lstat", "fstat", "exists",
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

        testFs("open/write/stat/read/close", async (fs) => {
            await fs.mkdir(baseDir);
            const fd = await fs.open(file, "w+", 0o666);
            assert.ok(fd);
            const writeBuf = new Uint8Array([0, 1, 2, 3, 4, 5, 6]);
            const readBuf = new Uint8Array(writeBuf.length);
            assert.equal(await fs.write(fd, writeBuf, 0, writeBuf.length, null), writeBuf.length);
            assert.isTrue(await fs.exists(fd));
            assert.equal((await fs.fstat(fd)).size, writeBuf.length);
            assert.equal(await fs.read(fd, readBuf, 0, readBuf.length, 0), readBuf.length);
            assert.deepEqual(readBuf, writeBuf);
            await fs.close(fd);
        });
    }

    mocha.run();
}

(window as any).runTests = runTests;
