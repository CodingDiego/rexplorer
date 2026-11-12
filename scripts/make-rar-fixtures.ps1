# Writes the encrypted RAR5 test archives under crates\core\tests\fixtures, byte by byte (no RAR tool needed).
# The format follows the RAR 5.0 technote; the key derivation follows unrar's crypt5.cpp: PBKDF2-HMAC-SHA256 with
# 2^lg2 iterations gives the AES-256 key, and 32 more iterations the value folded into the 8-byte password check.
#   enc-files.rar    plain headers; "readme.txt" stored plain, "secret.txt" encrypted with password "rexplorer-pw"
#   enc-headers.rar  headers and data encrypted with password "contrase" + U+00F1 + "a" (non-ASCII on purpose)
param([string]$Out = (Join-Path $PSScriptRoot '..\crates\core\tests\fixtures'))
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null

function VInt([uint64]$v) {
    $o = New-Object System.Collections.Generic.List[byte]
    do {
        $b = [byte]($v -band 0x7F); $v = $v -shr 7
        if ($v -ne 0) { $b = $b -bor 0x80 }
        $o.Add($b)
    } while ($v -ne 0)
    , $o.ToArray()
}

function Crc32([byte[]]$data) {
    # Int64 throughout: PowerShell 5.1 reads 0xFFFFFFFF as the int -1.
    [int64]$c = 4294967295
    foreach ($b in $data) {
        $c = $c -bxor $b
        for ($i = 0; $i -lt 8; $i++) { if ($c -band 1) { $c = ($c -shr 1) -bxor [int64]3988292384 } else { $c = $c -shr 1 } }
    }
    [uint32]($c -bxor [int64]4294967295)
}

function Concat { $l = New-Object System.Collections.Generic.List[byte]; foreach ($a in $args) { if ($null -ne $a) { $l.AddRange([byte[]]$a) } }; , $l.ToArray() }

# CRC32, size, body.
function Header([byte[]]$body) {
    $after = Concat (VInt $body.Length) $body
    Concat ([BitConverter]::GetBytes((Crc32 $after))) $after
}

function Pbkdf2([byte[]]$pwd, [byte[]]$salt, [int]$iter) {
    $k = New-Object System.Security.Cryptography.Rfc2898DeriveBytes($pwd, $salt, $iter, [System.Security.Cryptography.HashAlgorithmName]::SHA256)
    $k.GetBytes(32)
}

# 8-byte password check plus the first 4 bytes of its SHA-256.
function Check([byte[]]$pwd, [byte[]]$salt, [int]$lg2) {
    $v2 = Pbkdf2 $pwd $salt ((1 -shl $lg2) + 32)
    $chk = New-Object byte[] 8
    for ($i = 0; $i -lt 32; $i++) { $chk[$i % 8] = $chk[$i % 8] -bxor $v2[$i] }
    $sum = [System.Security.Cryptography.SHA256]::Create().ComputeHash($chk)
    Concat $chk $sum[0..3]
}

function Aes([byte[]]$key, [byte[]]$iv, [byte[]]$plain) {
    $pad = (16 - $plain.Length % 16) % 16
    $data = Concat $plain (New-Object byte[] $pad)
    $a = [System.Security.Cryptography.Aes]::Create()
    $a.Mode = 'CBC'; $a.Padding = 'None'; $a.Key = $key; $a.IV = $iv
    , $a.CreateEncryptor().TransformFinalBlock($data, 0, $data.Length)
}

function Bytes([int]$seed, [int]$n) { , [byte[]](1..$n | ForEach-Object { [byte](($seed * 31 + $_ * 7) % 256) }) }

$sig = [byte[]](0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00)
$main = Header ([byte[]](1, 0, 0))
$end = Header ([byte[]](5, 0, 0))
$lg2 = 4

# A stored file: type 2, flags (extra 0x01, data 0x02), sizes, file flags 0, unpacked size, attributes 0x20,
# compression 0 (stored), host 0 (Windows), name.
function FileHeader([string]$name, [int]$packed, [int]$unpacked, [byte[]]$extra) {
    $n = [Text.Encoding]::UTF8.GetBytes($name)
    if ($extra) {
        $body = Concat ([byte[]](2, 3)) (VInt $extra.Length) (VInt $packed) ([byte[]](0)) (VInt $unpacked) ([byte[]](0x20, 0, 0)) (VInt $n.Length) $n $extra
    } else {
        $body = Concat ([byte[]](2, 2)) (VInt $packed) ([byte[]](0)) (VInt $unpacked) ([byte[]](0x20, 0, 0)) (VInt $n.Length) $n
    }
    Header $body
}

# Record 0x01: version 0, flags 0x01 (password check present), lg2 count, salt, IV, check.
function CryptRecord([byte[]]$pwd, [byte[]]$salt, [byte[]]$iv) {
    $rec = Concat ([byte[]](1, 0, 1, $lg2)) $salt $iv (Check $pwd $salt $lg2)
    Concat (VInt $rec.Length) $rec
}

# enc-files.rar
$pw = [Text.Encoding]::UTF8.GetBytes('rexplorer-pw')
$salt = Bytes 1 16; $iv = Bytes 2 16
$secret = [Text.Encoding]::ASCII.GetBytes("top secret`r`n")
$cipher = Aes (Pbkdf2 $pw $salt (1 -shl $lg2)) $iv $secret
$readme = [Text.Encoding]::ASCII.GetBytes('plain')
$rar = Concat $sig $main (FileHeader 'readme.txt' $readme.Length $readme.Length $null) $readme `
    (FileHeader 'secret.txt' $cipher.Length $secret.Length (CryptRecord $pw $salt $iv)) $cipher $end
[IO.File]::WriteAllBytes((Join-Path $Out 'enc-files.rar'), $rar)

# enc-headers.rar: the archive encryption header (type 4: version 0, flags 0x01, lg2, salt, check) in clear, then
# each header as IV + AES(header padded to 16). The file's data has its own record and key.
$pw = [Text.Encoding]::UTF8.GetBytes('contrase' + [char]0xF1 + 'a')
$hsalt = Bytes 3 16
$hkey = Pbkdf2 $pw $hsalt (1 -shl $lg2)
$crypt = Header (Concat ([byte[]](4, 0, 0, 1, $lg2)) $hsalt (Check $pw $hsalt $lg2))
function Enc([byte[]]$h, [int]$seed) { $v = Bytes $seed 16; Concat $v (Aes $hkey $v $h) }
$fsalt = Bytes 4 16; $fiv = Bytes 5 16
$hidden = [Text.Encoding]::ASCII.GetBytes('hidden name, hidden data')
$fcipher = Aes (Pbkdf2 $pw $fsalt (1 -shl $lg2)) $fiv $hidden
$rar = Concat $sig $crypt (Enc $main 6) (Enc (FileHeader 'docs/hidden.txt' $fcipher.Length $hidden.Length (CryptRecord $pw $fsalt $fiv)) 7) $fcipher (Enc $end 8)
[IO.File]::WriteAllBytes((Join-Path $Out 'enc-headers.rar'), $rar)
Get-ChildItem $Out -Filter 'enc-*.rar' | ForEach-Object { '{0} {1} bytes' -f $_.Name, $_.Length }
