def __termeric_report_initial_cwd [path: string] {
    if not ($path | str starts-with '/') { return }
    if ($path | encode utf-8 | bytes length) > 4089 { return }
    let encoded = ($path | url encode --all | str replace --all '%2F' '/')
    if ($encoded | encode utf-8 | bytes length) > 4089 { return }
    print --no-newline $"(char --unicode '1b')]7;file://($encoded)(char --unicode '1b')\\"
}
if not $env.config.shell_integration.osc7 {
    __termeric_report_initial_cwd $env.PWD
}
$env.config.shell_integration.osc7 = true
$env.config.shell_integration.osc133 = true
