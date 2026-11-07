import { runAsOwner } from './helpers';

// The public API allows 5 bookings a minute per IP, counted in Postgres, and every test here books from 127.0.0.1:
// a rerun, or a retry, within the minute would be refused. Start each run with no hits counted.
export default function globalSetup() {
	runAsOwner(`
		const sql = new Bun.SQL(process.env.URL);
		await sql\`delete from ibe_rate_hit\`;
		await sql.close();
	`);
}
