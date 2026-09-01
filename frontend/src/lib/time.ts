export function formatDuration(timestampMs: number, future = false): string {
	const now = Date.now();
	const difference = future ? timestampMs - now : now - timestampMs;

	if (difference < 0) {
		return '0 seconds';
	}

	const seconds = Math.floor(difference / 1000);
	const minutes = Math.floor(seconds / 60);
	const hours = Math.floor(minutes / 60);
	const days = Math.floor(hours / 24);

	if (days > 0) return days === 1 ? '1 day' : `${days} days`;
	if (hours > 0) return hours === 1 ? '1 hour' : `${hours} hours`;
	if (minutes > 0) return minutes === 1 ? '1 minute' : `${minutes} minutes`;
	return seconds === 1 ? '1 second' : `${seconds} seconds`;
}
