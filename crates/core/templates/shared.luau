local Greeting = {}

function Greeting.forServer(): string
	return "Serveur prêt"
end

function Greeting.forPlayer(player: Player): string
	return "Bienvenue, " .. player.DisplayName
end

return Greeting
