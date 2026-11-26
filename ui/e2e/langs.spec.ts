// The language picker reads the registry (src/lib/i18n/langs.ts): one button per language, the page
// switches as a whole, and the choice survives a reload. A new language adds one row here.
import { expect, test } from '@playwright/test';

const LANGS = [
	{ code: 'es', label: 'ES', name: 'Español', group: 'Idioma', household: 'Hogar', beta: false },
	{ code: 'en', label: 'EN', name: 'English', group: 'Language', household: 'Household', beta: false },
	{ code: 'de', label: 'DE', name: 'Deutsch', group: 'Sprache', household: 'Haushalt', beta: true },
	{ code: 'fr', label: 'FR', name: 'Français', group: 'Langue', household: 'Foyer', beta: true },
	{ code: 'ca', label: 'CA', name: 'Català', group: 'Idioma', household: 'Llar', beta: true },
	{ code: 'it', label: 'IT', name: 'Italiano', group: 'Lingua', household: 'Nucleo familiare', beta: true },
	{ code: 'pt', label: 'PT', name: 'Português', group: 'Idioma', household: 'Agregado familiar', beta: true }
];
// A beta language says so in its button ("DE beta") and its tooltip ("Deutsch (beta)").
const button = (l: (typeof LANGS)[number]) => (l.beta ? `${l.label} beta` : l.label);

test('the picker lists every registry language and switches the page to each', async ({ page }) => {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(e.message));
	await page.goto(`/?lang=${LANGS[0].code}`);
	for (const l of [...LANGS.slice(1), LANGS[0]]) {
		const picker = page.locator('header .seg[role=group]');
		await expect(picker.getByRole('button')).toHaveText(LANGS.map(button));
		await picker.getByRole('button', { name: button(l), exact: true }).click();
		await expect(page.locator('html')).toHaveAttribute('lang', l.code);
		await expect(page.getByRole('group', { name: l.group })).toBeVisible();
		await expect(page.locator('.side .plabel').first()).toHaveText(l.household);
		for (const x of LANGS) {
			const b = picker.getByRole('button', { name: button(x), exact: true });
			await expect(b).toHaveAttribute('aria-pressed', String(x.code === l.code));
			await expect(b).toHaveAttribute('title', x.beta ? `${x.name} (beta)` : x.name);
		}
		// The choice is saved: a reload without ?lang keeps it.
		await page.goto('/');
		await expect(page.locator('html')).toHaveAttribute('lang', l.code);
	}
	expect(errors).toEqual([]);
});

// The language choices that reach the server send registry codes: the family invite (the new login's
// locale), the summary sheet, the next-draw sheet, a new share link and the gap letter. Each offers every language once, by code, named in itself.
test('the family invite offers every registry language by its code', async ({ page }) => {
	await page.goto('/family?lang=en');
	await page.locator('.ph .right .btn.pri').click();
	const invite = page.locator('form[aria-labelledby=fm-inv-h] select').first();
	await expect(invite).toBeVisible();
	const options = await invite.evaluate((s: HTMLSelectElement) => [...s.options].map((o) => [o.value, o.text.trim()]));
	expect(options).toEqual(LANGS.map((l) => [l.code, l.name]));
});

test('the summary sheet offers every registry language by its code', async ({ page }) => {
	const want = LANGS.map((l) => [l.code, l.name]);
	await page.goto('/summary?lang=en');
	const radios = page.locator('input[type=radio][name=lang]');
	await expect(radios).toHaveCount(LANGS.length);
	const values = await radios.evaluateAll((rs) => rs.map((r) => [(r as HTMLInputElement).value, r.parentElement?.textContent?.trim() ?? '']));
	expect(values).toEqual(want);
});

test('the next-draw sheet offers every registry language by its code', async ({ page }) => {
	await page.goto('/due/next-draw?lang=en');
	const radios = page.locator('input[type=radio][name=nd-lang]');
	await expect(radios).toHaveCount(LANGS.length);
	const values = await radios.evaluateAll((rs) => rs.map((r) => [(r as HTMLInputElement).value, r.parentElement?.textContent?.trim() ?? '']));
	expect(values).toEqual(LANGS.map((l) => [l.code, l.name]));
	// the sheet starts in the screen's language
	await expect(radios.and(page.locator(':checked'))).toHaveValue('en');
});

test('a new share link offers every registry language by its code', async ({ page }) => {
	await page.goto('/links?lang=es');
	const select = page.getByLabel('Idioma del resumen');
	await expect(select).toBeVisible();
	const options = await select.evaluate((s: HTMLSelectElement) => [...s.options].map((o) => [o.value, o.text.trim()]));
	expect(options).toEqual(LANGS.map((l) => [l.code, l.name]));
	await expect(select).toHaveValue('es');
});

test('the gap letter offers every registry language by its code', async ({ page }) => {
	await page.goto('/timeline?lang=en');
	const letter = page.locator('.letter').first();
	await expect(letter).toBeVisible();
	const chips = await letter
		.locator('a.chip')
		.evaluateAll((as) => as.map((a) => [new URL((a as HTMLAnchorElement).href).searchParams.get('lang'), a.textContent?.trim() ?? '', a.getAttribute('lang')]));
	expect(chips).toEqual(LANGS.map((l) => [l.code, l.name, l.code]));
});

// Longer words must not push text out of its card: the glance card header, the biomarker list, the
// buttons of a due item and the header picker, in every language, on a desktop and on a phone. On the
// phone the picker also keeps the beta tag visible, since a touch screen shows no tooltip. A tablet
// keeps a search field wide enough to type in.
const DESKTOP = { width: 1440, height: 900 };
const TABLETS = [
	{ width: 768, height: 1024 },
	{ width: 1024, height: 768 }
];
const PHONE = { width: 390, height: 844 };

for (const l of LANGS) {
	test(`nothing runs out of its card in ${l.code}`, async ({ page }) => {
		const out: string[] = [];
		await page.setViewportSize(DESKTOP);
		await page.goto(`/?lang=${l.code}`);
		await expect(page.locator('.sc').first()).toBeVisible();
		out.push(
			...(await page.locator('.sc').evaluateAll((cards) =>
				cards.flatMap((c) => {
					const edge = c.getBoundingClientRect().right;
					return [...c.querySelectorAll('.hd > *')]
						.filter((e) => e.getBoundingClientRect().right > edge + 0.5)
						.map((e) => `glance: "${e.textContent?.trim()}" ends at ${Math.round(e.getBoundingClientRect().right)} > card ${Math.round(edge)}`);
				})
			))
		);
		await page.goto(`/biomarkers?lang=${l.code}`);
		const list = page.locator('.tscroll:has(table.bml)');
		await expect(list).toBeVisible();
		const { scroll, client } = await list.evaluate((e) => ({ scroll: e.scrollWidth, client: e.clientWidth }));
		if (scroll > client + 1) out.push(`biomarkers: the list is ${scroll} px wide in a ${client} px card`);
		// the picker once left the search 4 px wide on an iPad held upright
		for (const vp of TABLETS) {
			await page.setViewportSize(vp);
			const w = await page.locator('header.top .search input').evaluate((e) => Math.round(e.getBoundingClientRect().width));
			if (w < 120) out.push(`tablet ${vp.width}: the search is ${w} px wide`);
		}

		await page.setViewportSize(PHONE);
		await page.goto(`/due?lang=${l.code}`);
		await expect(page.locator('.duerow').first()).toBeVisible();
		out.push(
			...(await page.locator('.duerow').evaluateAll((rows) =>
				rows.flatMap((r) => {
					const card = r.closest('.card') ?? r;
					const edge = card.getBoundingClientRect().right;
					return [...r.querySelectorAll('button')]
						.filter((b) => b.getBoundingClientRect().right > edge + 0.5)
						.map((b) => `due: "${b.textContent?.trim()}" ends at ${Math.round(b.getBoundingClientRect().right)} > card ${Math.round(edge)}`);
				})
			))
		);
		// the picker: every button has the same height, and the review count and the avatar stay on the
		// picker's row
		const head = await page.locator('header.top').evaluate((h) => {
			const bs = [...h.querySelectorAll('.seg[role=group] button')].map((b) => b.getBoundingClientRect());
			const mid = (e: Element | null) => {
				const r = e?.getBoundingClientRect();
				return r && Math.round(r.top + r.height / 2);
			};
			return { heights: bs.map((b) => Math.round(b.height)), top: Math.round(bs[0].top), bottom: Math.round(bs[0].bottom), review: mid(h.querySelector('a.iconbtn')), av: mid(h.querySelector('.av')) };
		});
		if (new Set(head.heights).size > 1) out.push(`picker: buttons of different heights ${head.heights}`);
		if (head.review === undefined || head.review < head.top || head.review > head.bottom) out.push(`picker: the review count is not on its row (${head.review} outside ${head.top}-${head.bottom})`);
		if (head.av !== undefined && (head.av < head.top || head.av > head.bottom)) out.push(`picker: the avatar is not on its row (${head.av} outside ${head.top}-${head.bottom})`);
		// a phone has no tooltip, so each beta language says beta in its button, readable and inside it
		const tags = await page.locator('header.top .seg[role=group] button').evaluateAll((bs) =>
			bs.map((b) => {
				const s = b.querySelector('small');
				const r = s?.getBoundingClientRect();
				const o = b.getBoundingClientRect();
				const inside = !!r && r.width >= 10 && r.height >= 7 && r.left >= o.left - 0.5 && r.right <= o.right + 0.5 && r.top >= o.top - 0.5 && r.bottom <= o.bottom + 0.5;
				// and painted: not hidden, not faded out, not a transparent colour
				const alpha = s ? Number(getComputedStyle(s).color.match(/[\d.]+/g)?.[3] ?? 1) : 0;
				const shown = inside && !!s?.checkVisibility({ visibilityProperty: true, opacityProperty: true }) && alpha > 0;
				return { label: b.textContent?.trim() ?? '', tag: s?.textContent ?? '', shown };
			})
		);
		for (const x of LANGS) {
			const t = tags.find((g) => g.label.startsWith(x.label));
			if (x.beta && !(t?.tag === 'beta' && t.shown)) out.push(`picker: ${x.label} does not show beta on a phone`);
			if (!x.beta && t?.tag) out.push(`picker: ${x.label} says ${t.tag}`);
		}
		expect(out).toEqual([]);
	});
}

// A notice puts the bell in the header too. Beside the picker and the bell, the search once shrank to
// 6 px at 1051 to 1100 px.
const NOTICES = {
	items: [
		{
			id: '00000000-0000-4000-8000-000000000001',
			kind: 'break_glass.request',
			profile: { id: '00000000-0000-4000-8000-000000000002', slug: 'alex', display_name: 'Alex Rivera' },
			request_id: '00000000-0000-4000-8000-000000000003',
			actor: { user_id: '00000000-0000-4000-8000-000000000004', display_name: 'Sam Fixture' },
			contact: { user_id: '00000000-0000-4000-8000-000000000004', display_name: 'Sam Fixture' },
			created_at: '2026-11-26T09:00:00Z',
			read_at: null
		}
	],
	uses: [],
	ice: [],
	unread: 1
};

for (const l of LANGS) {
	test(`with the bell shown, the header search stays wide enough to type in, in ${l.code}`, async ({ page }) => {
		await page.route('**/api/notifications', (route) => route.fulfill({ json: NOTICES }));
		await page.setViewportSize({ width: 1080, height: 900 });
		await page.goto(`/?lang=${l.code}`);
		await expect(page.locator('header.top .nt .iconbtn')).toBeVisible();
		const out: string[] = [];
		for (const width of [1051, 1080, 1100]) {
			await page.setViewportSize({ width, height: 900 });
			const w = await page.locator('header.top .search input').evaluate((e) => Math.round(e.getBoundingClientRect().width));
			if (w < 120) out.push(`${width} px with the bell: the search is ${w} px wide`);
		}
		expect(out).toEqual([]);
	});
}

// On a tablet with the bell shown, the review count, the bell and the avatar share the picker's row in
// every language. The avatar once wrapped to a row of its own at 768 px in de, fr and it.
for (const l of LANGS) {
	test(`with the bell shown, the avatar stays on the picker row on a tablet, in ${l.code}`, async ({ page }) => {
		await page.route('**/api/notifications', (route) => route.fulfill({ json: NOTICES }));
		await page.setViewportSize({ width: 768, height: 1024 });
		await page.goto(`/?lang=${l.code}`);
		await expect(page.locator('header.top .nt .iconbtn')).toBeVisible();
		await expect(page.locator('header.top .av')).toBeVisible();
		const out: string[] = [];
		for (const width of [761, 768, 820, 900, 1024, 1100]) {
			await page.setViewportSize({ width, height: 1024 });
			const head = await page.locator('header.top').evaluate((h) => {
				const box = (s: string) => h.querySelector(s)!.getBoundingClientRect();
				const row = box('.seg[role=group]');
				const mid = (s: string) => Math.round(box(s).top + box(s).height / 2);
				return { top: Math.round(row.top), bottom: Math.round(row.bottom), review: mid('a.iconbtn'), bell: mid('.nt .iconbtn'), av: mid('.av'), reviewH: Math.round(box('a.iconbtn').height), avW: Math.round(box('.av').width) };
			});
			for (const [what, y] of [['review count', head.review], ['bell', head.bell], ['avatar', head.av]] as const) {
				if (y < head.top || y > head.bottom) out.push(`${width} px: the ${what} is off the picker row (${y} outside ${head.top}-${head.bottom})`);
			}
			// the fix shortens a label, never a hit target
			if (head.reviewH < 34 || head.avW < 26) out.push(`${width} px: a hit target shrank (review ${head.reviewH} px high, avatar ${head.avW} px wide)`);
		}
		expect(out).toEqual([]);
	});
}

// The picker clips whatever spills past its rounded border, so a button's focus ring is drawn inside
// the button, where the clip cannot cut its top and bottom off.
test('the focus ring of a picker button is drawn whole', async ({ page }) => {
	await page.goto('/?lang=en');
	await expect(page.locator('.sc').first()).toBeVisible();
	const picker = page.locator('header .seg[role=group]');
	for (let i = 0; i < 100 && !(await picker.evaluate((g) => g.contains(document.activeElement))); i++) await page.keyboard.press('Tab');
	const ring = await picker.evaluate((g) => {
		const b = document.activeElement as HTMLElement;
		const st = getComputedStyle(b);
		return { inPicker: g.contains(b), focusVisible: b.matches(':focus-visible'), style: st.outlineStyle, reach: parseFloat(st.outlineOffset) + parseFloat(st.outlineWidth) };
	});
	expect(ring).toMatchObject({ inPicker: true, focusVisible: true, style: 'solid' });
	expect(ring.reach).toBeLessThanOrEqual(0);
});
