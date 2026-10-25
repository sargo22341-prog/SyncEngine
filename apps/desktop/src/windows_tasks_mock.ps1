# In-memory COM-shaped supervisor. Never connects to Schedule.Service.
function ErrorCode($exception) { while ($null -ne $exception.InnerException) { $exception=$exception.InnerException }; return $exception.HResult }
class MockActions {
    [int]$Count=1
    [object]$Value
    [object] Item([int]$index) { return $this.Value }
}
class MockTask {
    [string]$Name
    [object]$Definition
    [int]$State=3
    [bool]$Enabled=$true
    [object] GetInstances([int]$flags) { return [pscustomobject]@{Count=([int]($this.State -eq 4))} }
    [object] Run([object]$unused) { if (!$this.Enabled) { throw 'disabled task cannot start' }; $this.State=4; $script:Starts++; return $null }
}
function NewTask($name,$arguments) {
    $task=[MockTask]::new(); $task.Name=$name
    $actions=[MockActions]::new()
    $actions.Value=[pscustomobject]@{Path='C:\portable\syncengine-agent.exe';Arguments=$arguments}
    $task.Definition=[pscustomobject]@{Actions=$actions;Principal=[pscustomobject]@{UserId='S-1-5-21-1'; RunLevel=0; LogonType=3}}
    return $task
}
class MockFolder {
    [hashtable]$Tasks=@{}
    [bool]$Denied=$false
    [bool]$FailDeletion=$false
    [object] GetTask([string]$name) {
        if (!$this.Tasks.ContainsKey($name)) { throw [System.Runtime.InteropServices.COMException]::new('not found',-2147024894) }
        return $this.Tasks[$name]
    }
    [object] RegisterTask([string]$name,[string]$xml,[int]$flags,[string]$sid,[object]$password,[int]$logon,[object]$security) {
        if ($this.Denied) { throw [System.Runtime.InteropServices.COMException]::new('access denied',-2147024891) }
        [xml]$definition=$xml
        if (!$this.Tasks.ContainsKey($name)) { $this.Tasks[$name]=NewTask $name $definition.Task.Actions.Exec.Arguments }
        $this.Tasks[$name].Enabled=($flags -band 8) -eq 0
        return $this.Tasks[$name]
    }
    [void] DeleteTask([string]$name,[int]$flags) {
        if ($this.FailDeletion) { throw 'simulated interruption' }
        $this.Tasks.Remove($name)
    }
}
$script:Starts=0
$folder=[MockFolder]::new()
$folder.Tasks['SyncEngine-test-clock']=NewTask 'SyncEngine-test-clock' 'tick "C:\etat" clock'
$folder.Tasks['SyncEngine-test-logon']=NewTask 'SyncEngine-test-logon' 'tick "C:\etat" logon'
$folder.Tasks['SyncEngine-logon']=NewTask 'SyncEngine-logon' 'tick "C:\foreign" logon'
