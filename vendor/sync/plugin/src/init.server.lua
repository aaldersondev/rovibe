if not plugin then
	return
end

local Bridge = require(script.Bridge)

-- Studio loads every plugin into the server and client DataModels of each
-- playtest too. Syncing is the editor's job, so there the plugin starts the
-- bridge to the app, which agents need to act inside the running game, and
-- nothing else: mounting Roact and the whole panel in every test DataModel
-- costs seconds per playtest, enough on a busy machine to trip Studio's
-- plugin watchdog.
if game:GetService("RunService"):IsRunning() then
	Bridge.start(plugin)
	return
end

local Rojo = script:FindFirstAncestor("Rojo")
local Packages = Rojo.Packages

local Log = require(Packages.Log)
local Roact = require(Packages.Roact)

local Settings = require(script.Settings)
local Config = require(script.Config)
local App = require(script.App)

-- Started before the UI so that a failure to mount the panel doesn't also
-- cut agents off from Studio.
Bridge.start(plugin)

Log.setLogLevelThunk(function()
	return Log.Level[Settings:get("logLevel")] or Log.Level.Info
end)

local app = Roact.createElement(App, {
	plugin = plugin,
})
local tree = Roact.mount(app, game:GetService("CoreGui"), "Rojo UI")

plugin.Unloading:Connect(function()
	Roact.unmount(tree)
end)

if Config.isDevBuild then
	local TestEZ = require(script.Parent.TestEZ)

	require(script.runTests)(TestEZ)
end
