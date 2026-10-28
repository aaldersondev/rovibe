--[[
	Which RoVibe project a place belongs to: `ServerStorage.RoVibe`, attribute
	`ProjectId`. The app stamps it when it first syncs a project into the
	place, the plugin reports it when it connects, and a sync server started
	for another project is refused. Unlike a place id, it exists for a place
	that was never published, and unlike a file name it survives a rename.

	ServerStorage travels into a playtest's server DataModel, not its client.
]]
local ServerStorage = game:GetService("ServerStorage")

local NAME = "RoVibe"
local ATTRIBUTE = "ProjectId"

local PlaceMarker = {}

function PlaceMarker.get(): string?
	local marker = ServerStorage:FindFirstChild(NAME)
	if marker == nil then
		return nil
	end
	local ok, id = pcall(marker.GetAttribute, marker, ATTRIBUTE)
	if ok and type(id) == "string" and id ~= "" then
		return id
	end
	return nil
end

function PlaceMarker.set(id: string)
	local marker = ServerStorage:FindFirstChild(NAME)
	if marker == nil then
		marker = Instance.new("Configuration")
		marker.Name = NAME
		marker.Parent = ServerStorage
	end
	marker:SetAttribute(ATTRIBUTE, id)
end

return PlaceMarker
