if not plugin then
	return
end

local Rojo = script:FindFirstAncestor("Rojo")
local Packages = Rojo.Packages

local Log = require(Packages.Log)
local Roact = require(Packages.Roact)

local Settings = require(script.Settings)
local Config = require(script.Config)
local App = require(script.App)
local Bridge = require(script.Bridge)

-- Started before the UI so that a failure to mount the panel in one DataModel
-- doesn't also cut agents off from it.
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
