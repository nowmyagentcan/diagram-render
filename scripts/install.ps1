# Installs diagram-render @VERSION@ with its engines and fonts into %LOCALAPPDATA%\Programs\diagram-render after checking the archive's SHA-256.
# irm https://github.com/nowmyagentcan/diagram-render/releases/latest/download/install.ps1 | iex
& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'
  $version = '@VERSION@'
  $base = if ($env:DIAGRAM_RENDER_BASE) { $env:DIAGRAM_RENDER_BASE } else { "https://github.com/nowmyagentcan/diagram-render/releases/download/$version" }
  $archive = "diagram-render-$version-windows-x64.zip"
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    Invoke-WebRequest "$base/$archive" -OutFile "$tmp\$archive" -UseBasicParsing
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing
    $expected = Get-Content "$tmp\SHA256SUMS" | ForEach-Object { $hash, $name = -split $_; if ($name -eq $archive) { $hash } }
    if (-not $expected -or (Get-FileHash "$tmp\$archive" -Algorithm SHA256).Hash -ne $expected) {
      throw "diagram-render: the SHA-256 of $archive doesn't match SHA256SUMS; nothing was installed."
    }
    Expand-Archive "$tmp\$archive" "$tmp\x"
    $new = try { & "$tmp\x\diagram-render.exe" --version } catch { }
    if ("$new" -notlike 'diagram-render *') {
      throw "diagram-render: $archive has no diagram-render.exe that runs here; nothing was installed."
    }
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\diagram-render'
    New-Item -ItemType Directory -Force $dir | Out-Null
    # A running program (say an MCP client's server, or an engine it started) locks its file but
    # not its name, so each file moves aside, and back if the new version can't be copied; a copy
    # still running is removed by a later install.
    $placed = @()
    $moved = @()
    try {
      foreach ($file in Get-ChildItem "$tmp\x" -Recurse -File) {
        $to = Join-Path $dir $file.FullName.Substring("$tmp\x\".Length)
        New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
        if (Test-Path $to) {
          $old = "$to.$(Get-Random).old"
          Move-Item $to $old
          $moved += , @($old, $to)
        }
        $placed += $to
        Copy-Item $file.FullName $to
      }
    } catch {
      $placed | ForEach-Object { Remove-Item $_ -Force -ErrorAction SilentlyContinue }
      $moved | ForEach-Object { Move-Item -Force $_[0] $_[1] }
      throw
    }
    Get-ChildItem $dir -Recurse -File | Where-Object Name -Match '\.\d+\.old$' | Remove-Item -ErrorAction SilentlyContinue
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    $path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    if (($path -split ';') -notcontains $dir) {
      $key.SetValue('Path', "$path;$dir".TrimStart(';'), 'ExpandString')
      # Clearing a user variable makes Windows tell open programs that the environment changed.
      [Environment]::SetEnvironmentVariable('DIAGRAM_RENDER_INSTALL', $null, 'User')
      $env:Path += ";$dir"
    }
    & "$dir\diagram-render.exe" --version
  } finally {
    Remove-Item -Recurse -Force $tmp
  }
}
