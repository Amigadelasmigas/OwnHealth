// The Inbox screen for a member who manages no profile (a caregiver): sources and held mail belong to
// who manages a profile, so the page asks for no held mail (that list answers 403 to anyone else) and
// says the reader manages no sources here, in en and es, never "No sources yet".
import { expect, test, type Browser } from '@playwright/test';

/** As the demo owner (dev sign-in): invite a new person as Alex's caregiver, and sign them in. */
async function caregiver(browser: Browser, baseURL: string) {
	const owner = await browser.newContext({ baseURL, colorScheme: 'dark' });
	const headers = { Origin: baseURL };
	const invite = await owner.request.post('/api/household/invites', { headers, data: { display_name: `Pat Rivera ${Date.now()}`, locale: 'en' } });
	expect(invite.status()).toBe(201);
	const { user_id, code } = await invite.json();
	const grant = await owner.request.put('/api/grants', { headers, data: { user_id, profile: 'alex', role: 'caregiver' } });
	expect(grant.status(), await grant.text()).toBe(200);
	await owner.close();
	const ctx = await browser.newContext({ baseURL, colorScheme: 'dark' });
	const enrol = await ctx.request.post('/api/auth/enroll/password', { headers, data: { code, password: 'synthetic-pass-phrase-54' } });
	expect(enrol.status(), await enrol.text()).toBe(200);
	return ctx;
}

const TEXT = {
	en: "You manage no sources here. A profile's owner, or the guardian of a child's profile, adds and runs its sources.",
	es: 'Aquí no gestionas ningún origen. Los añade y los gestiona el titular del perfil, o quien tutela el perfil de un menor.'
};

test('a caregiver opening the Inbox asks for no held mail and reads that they manage no sources', async ({ browser, baseURL }) => {
	const ctx = await caregiver(browser, baseURL!);
	const page = await ctx.newPage();
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(e.message));
	page.on('console', (m) => {
		if (m.type() === 'error') errors.push(m.text());
	});
	const refused: string[] = [];
	const quarantine: string[] = [];
	page.on('response', (r) => {
		if (r.status() >= 400 && r.url().includes('/api/')) refused.push(`${r.status()} ${r.url()}`);
	});
	page.on('request', (r) => {
		if (r.url().includes('/api/inboxes/quarantine')) quarantine.push(r.url());
	});
	for (const lang of ['en', 'es'] as const) {
		await page.goto(`/inbox?lang=${lang}`);
		await expect(page.getByText(TEXT[lang])).toBeVisible();
		await expect(page.getByText(lang === 'en' ? 'No sources yet' : 'Aún no hay orígenes')).toHaveCount(0);
	}
	expect(quarantine).toEqual([]);
	expect(refused).toEqual([]);
	expect(errors).toEqual([]);
	await ctx.close();
});
