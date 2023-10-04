const S = require("./structs.js");
const c = require("./chains.js");
const _ = require("./helpers.js");

exports.init = function(volume, opts, bootSector) {
    if (bootSector[510] !== 0x55 || bootSector[511] !== 0xAA) throw Error("Invalid volume signature!");

    const isFAT16 = bootSector.readUInt16LE(S.boot16.fields["FATSz16"].offset);
    const bootStruct = (isFAT16) ? S.boot16 : S.boot32;
    const BS = bootStruct.valueFromBytes(bootSector);
    _.log(_.log.DBG, "Boot sector info:", BS);
    bootSector = null; // allow GC
    if (!BS.BytsPerSec) throw Error("This looks like an ExFAT volume! (unsupported)");
    else if (BS.BytsPerSec !== volume.sectorSize) throw Error("Sector size mismatch with FAT table.");

    const FATSz = (isFAT16) ? BS.FATSz16 : BS.FATSz32;
    const rootDirSectors = Math.ceil((BS.RootEntCnt * 32) / BS.BytsPerSec);
    const firstDataSector = BS.ResvdSecCnt + (BS.NumFATs * FATSz) + rootDirSectors;
    const totSec = (BS.TotSec16) ? BS.TotSec16 : BS.TotSec32;
    const dataSec = totSec - firstDataSector;
    const countofClusters = Math.floor(dataSec / BS.SecPerClus);
    // avoid corrupting sectors from other partitions or whatnot
    if (totSec > volume.numSectors) throw Error("Volume size mismatch!");

    let fatType;
    if (countofClusters < 4085) {
        fatType = "fat12";
    } else if (countofClusters < 65525) {
        fatType = "fat16";
    } else {
        fatType = "fat32";
    }

    _.log(_.log.DBG, "rootDirSectors", rootDirSectors, "firstDataSector", firstDataSector, "countofClusters", countofClusters, "=>", fatType);

    const vol = {};

    vol.opts = opts;

    vol._sectorSize = BS.BytsPerSec;
    vol._sectorsPerCluster = BS.SecPerClus;
    vol._firstSectorOfCluster = function(n) {
        return firstDataSector + (n-2)*vol._sectorsPerCluster;
    };

    vol._readSectors = function(secNum, dest, cb) {
        if (typeof dest === "function") {
            cb = dest;
            dest = _.allocBuffer(vol._sectorSize);
        }
        _.log(_.log.DBG, "vol._readSectors", secNum, dest.length);
        if (secNum < volume.numSectors) {
            volume.readSectors(secNum, dest, function(e) {
                cb(e, dest);
            });
        } else throw Error("Invalid sector number!");
    };

    vol._writeSectors = function(secNum, data, cb) {
        _.log(_.log.DBG, "vol._writeSectors", secNum, data.length);
        // NOTE: these are internal assertions, public API will get proper `S.err`s
        if (data.length % volume.sectorSize) throw Error("Buffer length not a multiple of sector size");
        else if (opts.ro) throw Error("Read-only filesystem");
        else if (secNum < volume.numSectors) volume.writeSectors(secNum, data, cb);
        else throw Error("Invalid sector number!");
    };

    function fatInfoForCluster(n) {
        const entryStruct = S.fatField[fatType];
        const FATOffset = (fatType === "fat12") ? Math.floor(n/2) * entryStruct.size : n * entryStruct.size;
        const SecNum = BS.ResvdSecCnt + Math.floor(FATOffset / BS.BytsPerSec);
        const EntOffset = FATOffset % BS.BytsPerSec;
        return { sector: SecNum-BS.ResvdSecCnt, offset: EntOffset, struct: entryStruct };
    }

    // TODO: all this FAT manipulation is crazy inefficient! needs read caching *and* write caching
    //        …the best place for cache might be in `volume` handler, though. add a `flush` method to that spec?
    // TODO: how should we handle redundant FATs? mirror every write? just ignore completely? copy-on-eject?

    const fatChain = c.sectorChain(vol, BS.ResvdSecCnt, FATSz);
    fatChain.cacheAdvice = "RANDOM";

    vol.fetchFromFAT = function(clusterNum, cb) {
        const info = fatInfoForCluster(clusterNum);
        fatChain.readFromPosition(info, info.struct.size, function(e, n, d) {
            if (e) return cb(e);
            let status = info.struct.valueFromBytes(d); var prefix;
            if (fatType === "fat12") {
                if (clusterNum % 2) {
                    status = (status.field1ab << 4) + status.field1c;
                } else {
                    status = (status.field0a << 8) + status.field0bc;
                }
            } else if (fatType === "fat32") {
                status &= 0x0FFFFFFF;
            }

            var prefix = S.fatPrefix[fatType];
            if (status === S.fatStat.free) cb(null, "free");
            else if (status === S.fatStat._undef) cb(null, "-invalid-");
            else if (status > prefix+S.fatStat.eofMin) cb(null, "eof");
            else if (status === prefix+S.fatStat.bad) cb(null, "bad");
            else if (status > prefix+S.fatStat.rsvMin) cb(null, "reserved");
            else cb(null, status);
        });
    };

    vol.storeToFAT = function(clusterNum, status, cb) {
        if (typeof status === "string") {
            status = S.fatStat[status];
            status += S.fatPrefix[fatType];
        }
        const info = fatInfoForCluster(clusterNum);
        // TODO: technically fat32 needs to *preserve* the high 4 bits
        if (fatType === "fat12") {
            fatChain.readFromPosition(info, info.struct.size, function(e, n, d) {
                const value = info.struct.valueFromBytes(d);
                if (clusterNum % 2) {
                    value.field1ab = status >>> 4;
                    value.field1c = status & 0x0F;
                } else {
                    value.field0a = status >>> 8;
                    value.field0bc = status & 0xFF;
                }
                const entry = info.struct.bytesFromValue(value);
                fatChain.writeToPosition(info, entry, cb);
            });
        } else {
            const entry = info.struct.bytesFromValue(status);
            fatChain.writeToPosition(info, entry, cb);
        }
    };

    vol.allocateInFAT = function(hint, cb) {
        if (typeof hint === "function") {
            cb = hint;
            hint = 2; // TODO: cache a better starting point?
        }
        function searchForFreeCluster(num, cb) {
            if (num < countofClusters) {
                vol.fetchFromFAT(num, function(e, status) {
                    if (e) cb(e);
                    else if (status === "free") cb(null, num);
                    else searchForFreeCluster(num+1, cb);
                });
            } else cb(S.err.NOSPC()); // TODO: try searching backwards from hint…
        }
        searchForFreeCluster(hint, function(e, clusterNum) {
            if (e) cb(e);
            else vol.storeToFAT(clusterNum, "eof", cb.bind(null, null, clusterNum));
        });
    };

    vol.rootDirectoryChain = (isFAT16) ?
        c.sectorChain(vol, firstDataSector - rootDirSectors, rootDirSectors) :
        c.clusterChain(vol, BS.RootClus);
    vol.rootDirectoryChain.cacheAdvice = "WILLNEED";
    vol.chainForCluster = c.clusterChain.bind(c, vol);
    vol.chainFromJSON = function(d) {
        return ("numSectors" in d) ?
            c.sectorChain(vol, d.firstSector, d.numSectors) :
            c.clusterChain(vol, d.firstCluster);
    };

    return vol;
};
