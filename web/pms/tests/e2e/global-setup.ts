import { createHash, createHmac } from 'node:crypto';

const RUSTFS = 'http://localhost:9000';
const BUCKETS = ['goodfolk-public', 'goodfolk-private'];
// RustFS's local development credentials (deploy/dev/rustfs.sh).
const ACCESS_KEY = 'goodfolk';
const SECRET_KEY = 'goodfolk_dev_secret';
const REGION = 'us-east-1';

const sha256 = (data: string) => createHash('sha256').update(data).digest('hex');
const hmac = (key: string | Buffer, data: string) =>
	createHmac('sha256', key).update(data).digest();

/**
 * HEAD on the bucket (S3 HeadBucket), signed with AWS Signature V4: 200 when it exists, 404 when it does not.
 * Signed, because RustFS answers an anonymous request for a missing bucket with 403, the same as for an existing
 * private one.
 */
async function headBucket(bucket: string): Promise<number> {
	const url = new URL(`/${bucket}`, RUSTFS);
	const amzDate = new Date().toISOString().replace(/[-:]|\.\d{3}/g, '');
	const day = amzDate.slice(0, 8);
	const payload = sha256('');
	const signedHeaders = 'host;x-amz-content-sha256;x-amz-date';
	const canonical = [
		'HEAD',
		url.pathname,
		'',
		`host:${url.host}\nx-amz-content-sha256:${payload}\nx-amz-date:${amzDate}\n`,
		signedHeaders,
		payload
	].join('\n');
	const scope = `${day}/${REGION}/s3/aws4_request`;
	const toSign = ['AWS4-HMAC-SHA256', amzDate, scope, sha256(canonical)].join('\n');
	const key = hmac(hmac(hmac(hmac(`AWS4${SECRET_KEY}`, day), REGION), 's3'), 'aws4_request');
	const signature = hmac(key, toSign).toString('hex');
	const response = await fetch(url, {
		method: 'HEAD',
		headers: {
			'x-amz-content-sha256': payload,
			'x-amz-date': amzDate,
			authorization: `AWS4-HMAC-SHA256 Credential=${ACCESS_KEY}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}`
		},
		signal: AbortSignal.timeout(2000)
	});
	return response.status;
}

// The media tests upload to RustFS, which Playwright cannot start. Playwright runs this once its web servers
// are up (neither needs RustFS to start) and before any test: without RustFS on :9000 and both buckets, stop here.
export default async function globalSetup() {
	let statuses: number[];
	try {
		statuses = await Promise.all(BUCKETS.map(headBucket));
	} catch {
		throw new Error('RustFS is not answering on :9000; start RustFS: deploy/dev/rustfs.sh start');
	}
	const missing = BUCKETS.filter((_, i) => statuses[i] === 404);
	if (missing.length > 0) {
		throw new Error(
			`RustFS has no ${missing.join(' or ')} bucket; run \`cargo run -p storage --example ensure-buckets\``
		);
	}
	// Anything but 200 (a 403 from wrong credentials or clock skew) would hide a missing bucket, so it stops here too.
	const refused = BUCKETS.filter((_, i) => statuses[i] !== 200);
	if (refused.length > 0) {
		throw new Error(
			`RustFS refused to show ${refused.join(' and ')} (${statuses.join(', ')}); check the dev credentials goodfolk / goodfolk_dev_secret`
		);
	}
}
