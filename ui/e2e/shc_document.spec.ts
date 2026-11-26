// A health card document (slice 22) says under its cards when rows of it wait for the reader. The note
// follows the review list: once the reader handles the last row, it goes, with no reload. The card is
// written into the flags of an uploaded bundle, and its waiting row is seeded with psql into the
// database serve.sh made, as the importer writes them.
import { expect, test, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';

const db = process.env.OWNHEALTH_E2E_DB ?? '';

/** One statement in the test database; the first line of its answer. */
function sql(q: string): string {
	return execFileSync('docker', ['exec', 'ownhealth-testpg', 'psql', '-U', 'postgres', '-d', db, '-qAtc', q], { encoding: 'utf8' }).trim();
}

function collectPageErrors(page: Page): string[] {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(e.message));
	return errors;
}

/** A card document of Alex's with one row waiting in review: (document id, review item id). */
async function seedCard(page: Page, id: string): Promise<{ doc: string; item: string }> {
	await page.goto('/import-review?lang=en');
	await page.evaluate(() => localStorage.setItem('oh.profile', 'alex'));
	const bundle = { resourceType: 'Bundle', id, type: 'collection', entry: [{ resource: { resourceType: 'Patient', id: 'alex-rivera', name: [{ family: 'Rivera', given: ['Alex'] }], birthDate: '1979-05-14' } }] };
	const sha = await page.evaluate(async (text) => {
		const form = new FormData();
		form.set('profile', 'alex');
		form.append('files', new Blob([text], { type: 'application/fhir+json' }), 'tarjeta LabNorte.json');
		const r = await fetch('/api/imports', { method: 'POST', body: form, headers: { accept: 'application/json' } });
		const body = await r.text();
		return (JSON.parse(body) as { items?: { sha256?: string }[] }).items?.[0]?.sha256 ?? `${r.status} ${body}`;
	}, JSON.stringify(bundle));
	expect(sha).toMatch(/^[0-9a-f]{64}$/);
	const docOf = () => sql(`select d.id from document d join profile p on p.id = d.profile_id where p.slug = 'alex' and d.blob_sha256 = decode('${sha}', 'hex')`);
	await expect.poll(docOf, { timeout: 20_000 }).toMatch(/^[0-9a-f-]{36}$/);
	const doc = docOf();
	const jobOf = `select state from job where kind = 'import.fhir_bundle' and payload->>'blob_sha256' = '${sha}'`;
	await expect.poll(() => sql(jobOf), { timeout: 20_000 }).toBe('done');
	// the FHIR tile of later checks names their own imports, not this one
	sql(`delete from job where kind = 'import.fhir_bundle' and payload->>'blob_sha256' = '${sha}'`);
	const card = { iss: 'https://issuer.labnorte.example', host: 'issuer.labnorte.example', kid: 'k'.repeat(43), issued: '2026-01-15T10:00:00Z', entries: [{ type: 'Immunization', date: '2025-10-01', text: 'Influenza' }] };
	sql(`update document set flags = flags || '${JSON.stringify({ shc: [card] })}'::jsonb where id = '${doc}'`);
	const payload = JSON.stringify({ projection: null, resource_type: 'Coverage', count: 1, notes: { 'stored, not projected': 1 }, source: { id: '00000000-0000-4000-8000-000000000022', kind: 'smart', label: 'Hospital LabNorte' }, import_refs: [`${id}/Coverage/1`] });
	const item = sql(
		`insert into review_item (household_id, profile_id, kind, document_id, reasons, payload) select household_id, id, 'fhir_unmapped_summary', '${doc}', '{unmapped}', '${payload}' from profile where slug = 'alex' returning id`
	);
	return { doc, item };
}

const NOTE = 'Rows that wait for you are under “Other items waiting”, below.';
const ISSUER = 'https://issuer.labnorte.example';
const isDoc = (doc: string) => (url: URL) => url.pathname === `/api/documents/${doc}`;

test('the waiting note of a card document goes once its last row is handled', async ({ page }) => {
	const errors = collectPageErrors(page);
	expect(db, 'the config names the database').not.toBe('');
	const { doc, item } = await seedCard(page, 'card-1');

	await page.goto(`/import-review?lang=en&doc=${doc}`);
	const note = page.getByText(NOTE);
	await expect(page.getByText(ISSUER)).toBeVisible();
	await expect(note).toBeVisible();
	const row = page.locator(`#oi-${item.slice(0, 8)}`);
	await row.getByRole('button', { name: 'Dismiss' }).click();
	await expect(row).toHaveCount(0);
	expect(sql(`select status from review_item where id = '${item}'`)).toBe('dismissed');
	// the card stays, and nothing waits for this reader any more
	await expect(page.getByText(ISSUER)).toBeVisible();
	await expect(note).toHaveCount(0);
	expect(errors).toEqual([]);
});

test('a row handled while the card document still loads leaves no stale note', async ({ page }) => {
	const errors = collectPageErrors(page);
	const { doc, item } = await seedCard(page, 'card-2');
	// the first load of the document answers as the server saw it before the dismissal, and only after it
	let release = () => {};
	const gate = new Promise<void>((r) => (release = r));
	let first = true;
	await page.route(isDoc(doc), async (route) => {
		if (!first) return route.continue();
		first = false;
		const stale = await route.fetch();
		await gate;
		await route.fulfill({ response: stale });
	});
	await page.goto(`/import-review?lang=en&doc=${doc}`);
	const row = page.locator(`#oi-${item.slice(0, 8)}`);
	await row.getByRole('button', { name: 'Dismiss' }).click();
	await expect(row).toHaveCount(0);
	expect(sql(`select status from review_item where id = '${item}'`)).toBe('dismissed');
	release();
	await expect(page.getByText(ISSUER)).toBeVisible();
	await expect(page.getByText(NOTE)).toHaveCount(0);
	expect(errors).toEqual([]);
});

test('a card document whose reload fails says so and offers a retry', async ({ page }) => {
	const errors = collectPageErrors(page);
	const { doc, item } = await seedCard(page, 'card-3');
	await page.goto(`/import-review?lang=en&doc=${doc}`);
	await expect(page.getByText(ISSUER)).toBeVisible();
	// one matcher for route and unroute: unroute drops only the matcher it is given
	const docUrl = isDoc(doc);
	await page.route(docUrl, (route) => route.fulfill({ status: 503, contentType: 'application/json', body: '{"detail":{"code":"unavailable","message":"down"}}' }));
	const row = page.locator(`#oi-${item.slice(0, 8)}`);
	await row.getByRole('button', { name: 'Dismiss' }).click();
	await expect(row).toHaveCount(0);
	const failed = page.getByRole('status').filter({ hasText: 'Could not load' });
	await expect(failed).toBeVisible();
	await page.unroute(docUrl);
	await failed.getByRole('button', { name: 'Retry' }).click();
	await expect(page.getByText(ISSUER)).toBeVisible();
	await expect(page.getByText(NOTE)).toHaveCount(0);
	expect(errors).toEqual([]);
});
