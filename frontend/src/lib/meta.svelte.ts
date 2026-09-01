import { getMeta } from './api';
import type { Meta } from './types';

let value = $state<Meta | null>(null);
let loading = $state(false);
let promise: Promise<Meta> | null = null;

export const metaState = {
	get value() {
		return value;
	},
	get loading() {
		return loading;
	}
};

// Memoized: every page that needs /meta calls this, but it only ever fetches once.
export function loadMeta(): Promise<Meta> {
	if (promise) return promise;

	loading = true;
	promise = getMeta()
		.then((meta) => {
			value = meta;
			return meta;
		})
		.finally(() => {
			loading = false;
		});

	return promise;
}
