export function forServer(): string {
	return "Serveur prêt";
}

export function forPlayer(player: Player): string {
	return `Bienvenue, ${player.DisplayName}`;
}
