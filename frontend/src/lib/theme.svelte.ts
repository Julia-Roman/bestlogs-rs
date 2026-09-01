export type Theme = 'light' | 'dark';

let theme = $state<Theme>('dark');

export const themeState = {
	get value() {
		return theme;
	}
};

function apply(next: Theme) {
	theme = next;
	document.documentElement.classList.toggle('dark', next === 'dark');
	try {
		localStorage.setItem('theme', next);
	} catch {
		// storage may be unavailable (private mode, blocked); theme just won't persist
	}
}

/** Reads the theme the boot script in app.html already applied to <html>. */
export function initTheme() {
	theme = document.documentElement.classList.contains('dark') ? 'dark' : 'light';
}

export function toggleTheme() {
	apply(theme === 'dark' ? 'light' : 'dark');
}
