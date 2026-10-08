$ErrorActionPreference = 'Stop'
$marker = $args[0]
$listMarker = $args[1]
$sessionValue = ''

while ($null -ne ($line = [Console]::ReadLine())) {
    try {
        $request = $line | ConvertFrom-Json
        if ($null -eq $request.id) { continue }
        $id = $request.id
        switch ($request.method) {
            'initialize' {
                $result = @{
                    protocolVersion = $request.params.protocolVersion
                    capabilities = @{ tools = @{} }
                    serverInfo = @{ name = 'khaslana-workflow-test'; version = '1.0.0' }
                }
            }
            'tools/list' {
                if ($listMarker) { [System.IO.File]::AppendAllText($listMarker, "list`n") }
                $valueSchema = @{
                    type = 'object'
                    properties = @{ value = @{ type = 'string' } }
                    required = @('value')
                    additionalProperties = $false
                }
                $echoOutputSchema = @{
                    type = 'object'
                    properties = @{ value = @{ type = 'string' } }
                    required = @('value')
                }
                $result = @{ tools = @(
                    @{ name = 'echo'; inputSchema = $valueSchema; outputSchema = $echoOutputSchema },
                    @{ name = 'write_file'; inputSchema = $valueSchema },
                    @{ name = 'set_state'; inputSchema = $valueSchema },
                    @{ name = 'get_state'; inputSchema = @{ type = 'object' }; outputSchema = $echoOutputSchema },
                    @{ name = 'stall'; inputSchema = @{ type = 'object' } },
                    @{ name = 'browser_navigate'; inputSchema = @{ type = 'object'; properties = @{ url = @{ type = 'string' } }; required = @('url') } },
                    @{ name = 'browser_snapshot'; inputSchema = @{ type = 'object' } },
                    @{ name = 'browser_type'; inputSchema = @{ type = 'object'; properties = @{ target = @{ type = 'string' }; text = @{ type = 'string' } }; required = @('target', 'text') } },
                    @{ name = 'new_page'; inputSchema = @{ type = 'object'; properties = @{ url = @{ type = 'string' } }; required = @('url') } },
                    @{ name = 'take_snapshot'; inputSchema = @{ type = 'object'; properties = @{ pageId = @{ type = 'integer' } }; required = @('pageId') } },
                    @{ name = 'fill'; inputSchema = @{ type = 'object'; properties = @{ pageId = @{ type = 'integer' }; uid = @{ type = 'string' }; value = @{ type = 'string' } }; required = @('pageId', 'uid', 'value') } }
                ) }
            }
            'tools/call' {
                switch ($request.params.name) {
                    'echo' {
                        $result = @{ content = @(); structuredContent = @{ value = $request.params.arguments.value } }
                    }
                    'write_file' {
                        [System.IO.File]::WriteAllText($marker, [string]$request.params.arguments.value)
                        $result = @{ content = @(); structuredContent = @{ written = $true } }
                    }
                    'set_state' {
                        $sessionValue = [string]$request.params.arguments.value
                        $result = @{ content = @(); structuredContent = @{ saved = $true } }
                    }
                    'get_state' {
                        $result = @{ content = @(); structuredContent = @{ value = $sessionValue } }
                    }
                    'stall' {
                        Start-Sleep -Seconds 60
                        $result = @{ content = @(); structuredContent = @{ done = $true } }
                    }
                    'browser_navigate' {
                        $sessionValue = [string]$request.params.arguments.url
                        $result = @{ content = @(@{ type = 'text'; text = "Page URL: $sessionValue" }) }
                    }
                    'browser_snapshot' {
                        $page = if ($marker -like '*wrong-page*') { 'Wrong page' } else { 'Web form Text input' }
                        if ($sessionValue -and (Test-Path -LiteralPath $marker)) {
                            $page += ' ' + [System.IO.File]::ReadAllText($marker)
                        }
                        $result = @{ content = @(@{ type = 'text'; text = $page }) }
                    }
                    'browser_type' {
                        [System.IO.File]::WriteAllText($marker, [string]$request.params.arguments.text)
                        $result = @{ content = @(@{ type = 'text'; text = 'filled' }) }
                    }
                    'new_page' {
                        $sessionValue = [string]$request.params.arguments.url
                        $result = @{ content = @(@{ type = 'text'; text = "Pages: 1: $sessionValue [selected]" }) }
                    }
                    'take_snapshot' {
                        $value = if (Test-Path -LiteralPath $marker) { [System.IO.File]::ReadAllText($marker) } else { '' }
                        $result = @{ content = @(@{ type = 'text'; text = "uid=1_0 RootWebArea Web form url=$sessionValue`nuid=1_1 textbox Text input value=$value" }) }
                    }
                    'fill' {
                        if ($request.params.arguments.pageId -ne 1 -or $request.params.arguments.uid -ne '1_1') { throw 'wrong target' }
                        [System.IO.File]::WriteAllText($marker, [string]$request.params.arguments.value)
                        $result = @{ content = @(@{ type = 'text'; text = 'filled Text input' }) }
                    }
                    default { throw 'unknown tool' }
                }
            }
            default { throw 'unknown method' }
        }
        $response = @{ jsonrpc = '2.0'; id = $id; result = $result }
        [Console]::Out.WriteLine(($response | ConvertTo-Json -Depth 30 -Compress))
        [Console]::Out.Flush()
    } catch {
        if ($null -ne $id) {
            $response = @{ jsonrpc = '2.0'; id = $id; error = @{ code = -32603; message = 'test server error' } }
            [Console]::Out.WriteLine(($response | ConvertTo-Json -Depth 10 -Compress))
            [Console]::Out.Flush()
        }
    }
}
