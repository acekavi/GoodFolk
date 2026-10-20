/**
 * ISO 4217 minor units, as the server's `fx::exponent`: 0, 3 for thousandths, else 2. The one source of a
 * currency's decimals for folios and rates alike.
 */
export function exponent(currency: string): number {
	if (
		[
			'BIF',
			'CLP',
			'DJF',
			'GNF',
			'ISK',
			'JPY',
			'KMF',
			'KRW',
			'PYG',
			'RWF',
			'UGX',
			'VND',
			'VUV',
			'XAF',
			'XOF',
			'XPF'
		].includes(currency)
	)
		return 0;
	if (['BHD', 'IQD', 'JOD', 'KWD', 'LYD', 'OMR', 'TND'].includes(currency)) return 3;
	return 2;
}
