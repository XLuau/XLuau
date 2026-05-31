local SERVICE_NAME = "RunService"
local ADAPTER = "task"
local endpoint = "/api/v1"
local propertySet = table.freeze({CFrame = true, Size = true})
return endpoint, SERVICE_NAME, ADAPTER, propertySet
