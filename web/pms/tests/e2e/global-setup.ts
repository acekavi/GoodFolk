// The media tests upload to RustFS, which Playwright cannot start. Playwright runs this once its web servers
// are up (neither needs RustFS to start) and before any test: without RustFS on :9000, stop here.
export default async function globalSetup() {
	try {
		// Any answer, even 403, means the server is up.
		await fetch('http://localhost:9000/', { signal: AbortSignal.timeout(2000) });
	} catch {
		throw new Error('RustFS is not answering on :9000; start RustFS: deploy/dev/rustfs.sh start');
	}
}
