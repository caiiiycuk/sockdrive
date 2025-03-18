
const dns = require('node:dns');
dns.setDefaultResultOrder('ipv4first');

const assert = require('assert');
const { sockdrive, OpResult } = require('../dist/sockdrive.cjs.js');

async function common(url: string) {

	async function connect_success() {
		const drive = await sockdrive(url);
		assert.ok(drive);
	};

	async function read_write_test() {
		const drive = await sockdrive(url);
		// read real range
		assert.ok(!drive.info.dropped_ranges.includes(0));
		let opResult = drive.readRangeSync(0);
		assert.ok(opResult.result === OpResult.AGAIN);

		opResult = await drive.readRangeAsync(0);
		assert.ok(opResult.result === OpResult.OK);
		assert.equal(opResult.data.length, drive.info.ahead_read);
		assert.equal(opResult.data[0], 235);
		assert.ok(opResult.data[1] === 60 || opResult.data[1] === 88);
		assert.ok(opResult.data[2] === 144);

		opResult = drive.readRangeSync(0);
		assert.ok(opResult.result === OpResult.OK);
		assert.equal(opResult.data.length, drive.info.ahead_read);
		assert.equal(opResult.data[0], 235);
		assert.ok(opResult.data[1] === 60 || opResult.data[1] === 88);
		assert.equal(opResult.data[2], 144);

		const payload = new Uint8Array(drive.info.sector_size);
		payload[0] = 1;
		payload[1] = 2;
		payload[2] = 3;
		payload[drive.info.sector_size - 1] = 4;
		drive.write(0, payload);
		
		opResult = drive.readRangeSync(0);
		assert.ok(opResult.result === OpResult.OK);
		assert.equal(opResult.data.length, drive.info.ahead_read);
		assert.equal(opResult.data[0], 1);
		assert.equal(opResult.data[1], 2);
		assert.equal(opResult.data[2], 3);
		assert.equal(opResult.data[drive.info.sector_size - 1], 4);

		// read dropped range
		assert.ok(drive.info.dropped_ranges.includes(1));
		opResult = drive.readRangeSync(1);
		assert.ok(opResult.result === OpResult.OK);
		assert.equal(opResult.data.length, drive.info.ahead_read);
		assert.equal(opResult.data[0], 0);
		assert.equal(opResult.data[1], 0);
		assert.equal(opResult.data[2], 0);
		assert.equal(opResult.data[drive.info.sector_size - 1], 0);

		drive.write(0, payload);
		opResult = drive.readRangeSync(0);
		assert.ok(opResult.result === OpResult.OK);
		assert.equal(opResult.data.length, drive.info.ahead_read);
		assert.equal(opResult.data[0], 1);
		assert.equal(opResult.data[1], 2);
		assert.equal(opResult.data[2], 3);
		assert.equal(opResult.data[drive.info.sector_size - 1], 4);
	};

	await connect_success();
	await read_write_test();
}

async function run() {
	try {
		await fetch("http://localhost:8080");
	} catch (e) {
		throw new Error("You need to start the server first");
	}

	async function connect_failure() {
		try {
			await sockdrive("http://localhost:8080/not-a-real-address");
			assert.fail("Should have failed");
		} catch (e) {
			// Ok
		}
	};

	await connect_failure();
	await common("http://localhost:8080/fat16-256mb");
	await common("http://localhost:8080/fat32-2gb");
}

// run().catch(console.error);

(async () => {
	const expect = await (await fetch("http://localhost:8080/win95-v1.json")).json();
	const drive = await sockdrive("http://localhost:8080/win95-v1");
	const bytes: Uint8Array= (await drive.readRangeAsync(0)).data;
	for (let i = 0; i < bytes.length; ++i) {
		assert.equal(expect[i], bytes[i]);
	}
})().catch(console.error);