import type { ContactMeta, Meta, StatusMeta } from './types';

async function getJson<T>(path: string): Promise<T> {
	const res = await fetch(path);
	if (!res.ok) {
		throw new Error(`${path} responded with ${res.status}`);
	}
	return (await res.json()) as T;
}

export const getMeta = () => getJson<Meta>('/meta');
export const getContactMeta = () => getJson<ContactMeta>('/meta/contact');
export const getStatusMeta = () => getJson<StatusMeta>('/meta/status');
