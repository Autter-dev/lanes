# Install Lanes (lane.exe and laned.exe) from GitHub Releases on Windows.
#
#   irm https://github.com/Autter-dev/lanes/releases/latest/download/lanes-installer.ps1 | iex
#
# Options, as environment variables set before you run it:
#   $env:LANES_VERSION = '0.1.0'       install this version instead of the one this script came with
#   $env:LANES_INSTALL_DIR = 'C:\bin'  where to put the binaries (default: %LOCALAPPDATA%\Programs\Lanes\bin)
#   $env:LANES_NO_MODIFY_PATH = '1'    don't add that folder to your user PATH
#
# It downloads one archive and its checksum from github.com, checks the checksum, copies two files
# and adds their folder to your user PATH. It sends nothing anywhere else. Run it again to update.
# To uninstall, delete the folder it printed and remove it from your PATH.

& {
    $ErrorActionPreference = 'Stop'
    # Invoke-WebRequest is far faster without its progress bar.
    $ProgressPreference = 'SilentlyContinue'

    $Repo = 'Autter-dev/lanes'
    # The release workflow stamps the release's version in here.
    $DefaultVersion = '__LANES_VERSION__'

    function Fail([string]$Message) { throw "lanes: $Message" }
    function Say([string]$Message) { Write-Host "lanes: $Message" }

    $version = $env:LANES_VERSION
    if (-not $version) {
        if ($DefaultVersion.StartsWith('__')) {
            Fail "this copy of the installer has no version in it. Use the one from a release (https://github.com/$Repo/releases/latest/download/lanes-installer.ps1), or set `$env:LANES_VERSION = '0.1.0' first."
        }
        $version = $DefaultVersion
    }
    $version = $version.TrimStart('v')

    # A 32-bit PowerShell on 64-bit Windows reports the real CPU in PROCESSOR_ARCHITEW6432.
    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { $target = 'x86_64-pc-windows-msvc' }
        'ARM64' { $target = 'aarch64-pc-windows-msvc' }
        default { Fail "Lanes has no build for $arch processors yet, only x64 and arm64. See https://lanes.autter.dev/download." }
    }

    $dir = $env:LANES_INSTALL_DIR
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'Programs\Lanes\bin' }

    $name = "lanes-$version-$target"
    $url = "https://github.com/$Repo/releases/download/v$version/$name.zip"
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("lanes-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null

    try {
        # Windows PowerShell 5.1 doesn't offer TLS 1.2 by default.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

        Say "downloading Lanes $version for $target"
        $zip = Join-Path $tmp "$name.zip"
        try {
            Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
            Invoke-WebRequest -Uri "$url.sha256" -OutFile "$zip.sha256" -UseBasicParsing
        } catch {
            Fail "couldn't download $url. Check your connection, and that version $version is listed at https://github.com/$Repo/releases."
        }

        $expected = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($expected -ne $actual) {
            Fail "the download doesn't match its checksum, so nothing was installed. Run the installer again; if it keeps happening, open an issue at https://github.com/$Repo/issues."
        }

        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        foreach ($exe in 'lane.exe', 'laned.exe') {
            $src = Join-Path $tmp "$name\$exe"
            if (-not (Test-Path $src)) { Fail "the archive has no $exe in it. Please open an issue at https://github.com/$Repo/issues." }
            # Write a temporary file and rename it, so a running copy is never left half-written.
            $new = Join-Path $dir "$exe.new"
            Copy-Item $src $new -Force
            try {
                Move-Item $new (Join-Path $dir $exe) -Force
            } catch {
                Remove-Item $new -Force -ErrorAction SilentlyContinue
                Fail "couldn't replace $dir\$exe, probably because it's running. Close lane and laned (Stop-Process -Name lane, laned), then run the installer again."
            }
        }
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    Say "installed lane and laned $version in $dir"

    # Read the user PATH without expanding %VARIABLES%, and write it back the same way.
    $key = 'HKCU:\Environment'
    $raw = (Get-Item -Path $key).GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $parts = @($raw -split ';' | Where-Object { $_ })
    if ($parts -contains $dir) {
        Say 'run `lane` in a repo to start'
    } elseif ($env:LANES_NO_MODIFY_PATH) {
        Say "$dir isn't on your PATH. Add it to run lane from anywhere."
    } else {
        Set-ItemProperty -Path $key -Name Path -Value (($parts + $dir) -join ';') -Type ExpandString
        # Setting any user variable through .NET tells open programs that the environment changed.
        [Environment]::SetEnvironmentVariable('LANES_INSTALLER', '1', 'User')
        [Environment]::SetEnvironmentVariable('LANES_INSTALLER', $null, 'User')
        $env:Path = "$env:Path;$dir"
        Say "added $dir to your user PATH. Run ``lane`` in a repo to start; other open terminals need restarting to see it."
    }
}
