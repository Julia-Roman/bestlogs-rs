export interface UmamiMeta {
	url: string;
	id: string;
}

export interface Meta {
	version: string;
	commit: string;
	instances: string[];
	umami: UmamiMeta | null;
}

export interface PersonInfo {
	name: string;
	login: string;
	avatar: string;
	id: string;
}

export interface ContactMeta {
	creator: PersonInfo;
}

export interface StatusInstance {
	maintainer?: string;
	channels: number;
	up: boolean;
}

export interface StatusMeta {
	instances: Record<string, StatusInstance>;
	lastUpdate: number;
	nextUpdate: number;
	uptime: number;
}
