import { intlLocale, isLang } from '../i18n/langs.ts';

/** The dict key for a count: `key_one` when the language says "one" and that key exists, else `key`.
 *  A registry language asks the plural rules of its own locale (`pt` is pt-PT, where 0 is not "one"). */
export function pluralKey(lang: string, n: number, key: string, has: (k: string) => boolean): string {
	const cat = new Intl.PluralRules(isLang(lang) ? intlLocale(lang) : lang).select(n);
	return cat === 'one' && has(key + '_one') ? key + '_one' : key;
}
