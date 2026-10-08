--[[
	Keeps a WebSocket open to the RoVibe desktop app so that agents can drive
	Studio (run code, read the tree, start a playtest) with one round trip per
	call instead of the HTTP long-polling other bridges rely on.

	This module is only the transport. What each call does comes from the app,
	which sends its tool implementations as a chunk of Luau on every
	connection: a plugin file is only read when Studio starts, and restarting
	Studio to pick up a change costs the user whatever they hadn't saved.

	The plugin is loaded once per DataModel, so during a playtest the server
	and client DataModels each open their own connection and report their
	context. The app picks the connection matching the context it wants.
]]
local HttpService = game:GetService("HttpService")
local LogService = game:GetService("LogService")
local RunService = game:GetService("RunService")

local DEFAULT_PORT = 34880
-- Version of the transport itself, so the app can tell an outdated plugin
-- from one that merely needs its methods.
local PROTOCOL_VERSION = 2
local RETRY_SECONDS = 4
local LOG_FLUSH_SECONDS = 0.1
-- A runaway print loop must not turn into an unbounded WebSocket backlog.
local MAX_PENDING_LOGS = 2000
local MAX_LOG_LENGTH = 4000

local LOG_LEVELS = {
	[Enum.MessageType.MessageOutput] = "output",
	[Enum.MessageType.MessageInfo] = "info",
	[Enum.MessageType.MessageWarning] = "warn",
	[Enum.MessageType.MessageError] = "error",
}

local Bridge = {}

local running = false
local client = nil
local syncController = nil
local pendingLogs = {}
local droppedLogs = 0
local methods = {}

local function getContext()
	if RunService:IsEdit() then
		return "edit"
	elseif RunService:IsClient() and not RunService:IsServer() then
		return "client"
	end
	return "server"
end

local env = {
	getSyncController = function()
		return syncController
	end,
}

-- A ModuleScript built on the fly is the one way to compile code that works
-- in every DataModel: loadstring is off in a running server and absent on
-- the client.
local function loadMethods(params)
	local module = Instance.new("ModuleScript")
	module.Name = "RoVibeMethods"
	module.Source = params.source

	local build = require(module)
	methods = build(env)
	return "loaded"
end

local function send(payload)
	if client then
		pcall(client.Send, client, HttpService:JSONEncode(payload))
	end
end

local function handleMessage(raw)
	local ok, message = pcall(HttpService.JSONDecode, HttpService, raw)
	if not ok or type(message) ~= "table" or message.id == nil then
		return
	end

	local handler = if message.method == "__load" then loadMethods else methods[message.method]
	if not handler then
		send({ id = message.id, ok = false, error = "Unknown method " .. tostring(message.method) })
		return
	end

	task.spawn(function()
		local success, result = pcall(handler, message.params or {})
		if success then
			send({ id = message.id, ok = true, result = result })
		else
			send({ id = message.id, ok = false, error = tostring(result) })
		end
	end)
end

local function connectOnce(port)
	local url = string.format(
		"ws://127.0.0.1:%d/studio?v=%d&placeId=%d&context=%s&name=%s",
		port,
		PROTOCOL_VERSION,
		game.PlaceId,
		getContext(),
		HttpService:UrlEncode(game.Name)
	)

	local ok, created = pcall(HttpService.CreateWebStreamClient, HttpService, Enum.WebStreamClientType.WebSocket, {
		Url = url,
	})
	if not ok then
		return
	end

	local closed = false
	client = created

	local connections = {
		created.MessageReceived:Connect(handleMessage),
		created.Closed:Connect(function()
			closed = true
		end),
		created.Error:Connect(function()
			closed = true
		end),
	}

	while running and not closed do
		task.wait(0.25)
	end

	for _, connection in connections do
		connection:Disconnect()
	end
	client = nil
	pcall(created.Close, created)
end

function Bridge.setSyncController(controller)
	syncController = controller
end

function Bridge.start(plugin)
	if running then
		return
	end
	running = true

	local port = tonumber(plugin:GetSetting("RoVibeBridgePort")) or DEFAULT_PORT

	if RunService:IsRunning() then
		-- Game scripts start before plugins do, so their first prints would
		-- otherwise never reach the app.
		for _, entry in LogService:GetLogHistory() do
			table.insert(pendingLogs, {
				l = LOG_LEVELS[entry.messageType] or "output",
				m = string.sub(entry.message, 1, MAX_LOG_LENGTH),
			})
		end
	end

	local logConnection = LogService.MessageOut:Connect(function(message, messageType)
		if #pendingLogs >= MAX_PENDING_LOGS then
			droppedLogs += 1
			return
		end
		table.insert(pendingLogs, { l = LOG_LEVELS[messageType] or "output", m = string.sub(message, 1, MAX_LOG_LENGTH) })
	end)

	task.spawn(function()
		while running do
			task.wait(LOG_FLUSH_SECONDS)
			if client and (#pendingLogs > 0 or droppedLogs > 0) then
				local entries = pendingLogs
				pendingLogs = {}
				if droppedLogs > 0 then
					table.insert(entries, { l = "warn", m = string.format("[rovibe] %d log lines dropped", droppedLogs) })
					droppedLogs = 0
				end
				send({ event = "log", entries = entries })
			end
		end
	end)

	task.spawn(function()
		while running do
			connectOnce(port)
			if running then
				task.wait(RETRY_SECONDS)
			end
		end
	end)

	plugin.Unloading:Connect(function()
		running = false
		logConnection:Disconnect()
	end)
end

return Bridge
