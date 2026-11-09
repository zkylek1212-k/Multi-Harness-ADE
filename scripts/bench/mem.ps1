# Working Set (MB) of a process tree. app = the app and its browser processes; agent = terminals and fake CLIs.
# Keep this file ASCII: Windows PowerShell 5.1 reads scripts without a BOM as the ANSI codepage.
param([Parameter(Mandatory = $true)][int]$RootPid)
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, WorkingSetSize
$kids = @{}
foreach ($p in $all) {
  $parent = [int]$p.ParentProcessId
  if (-not $kids.ContainsKey($parent)) { $kids[$parent] = @() }
  $kids[$parent] += $p
}
$app = 0; $agent = 0; $appPrivate = 0; $agentPrivate = 0; $count = 0
$stack = New-Object System.Collections.Stack
foreach ($p in $all) { if ([int]$p.ProcessId -eq $RootPid) { $stack.Push($p) } }
while ($stack.Count -gt 0) {
  $p = $stack.Pop()
  $count++
  $private = 0
  try { $private = (Get-Process -Id ([int]$p.ProcessId) -ErrorAction Stop).PrivateMemorySize64 } catch {}
  if ($p.Name -match '^(node|OpenConsole|conhost|cmd|pwsh|powershell)\.exe$') { $agent += $p.WorkingSetSize; $agentPrivate += $private } else { $app += $p.WorkingSetSize; $appPrivate += $private }
  $id = [int]$p.ProcessId
  if ($kids.ContainsKey($id)) { foreach ($c in $kids[$id]) { if ([int]$c.ProcessId -ne $id) { $stack.Push($c) } } }
}
@{ app_mb = [math]::Round($app / 1MB, 1); agent_mb = [math]::Round($agent / 1MB, 1); app_private_mb = [math]::Round($appPrivate / 1MB, 1); agent_private_mb = [math]::Round($agentPrivate / 1MB, 1); processes = $count } | ConvertTo-Json -Compress
