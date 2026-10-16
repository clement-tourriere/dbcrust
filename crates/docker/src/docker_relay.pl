use strict;
use warnings;
use IO::Select;

# IO::Socket::IP (core since perl 5.20) speaks IPv4 and IPv6; older perls
# only get IPv4 from IO::Socket::INET.
my $class = eval { require IO::Socket::IP; 1 }
    ? 'IO::Socket::IP'
    : do { require IO::Socket::INET; 'IO::Socket::INET' };

$SIG{PIPE} = 'IGNORE';

# Byte relay between this process's stdin/stdout and a TCP endpoint.
# Usage: relay ADDRESS PORT   (an IP literal and a port, validated; never
# evaluated as code)
my ($host, $port) = @ARGV;
die "usage: relay HOST PORT\n"
    unless defined $port
    && $host =~ /\A[0-9A-Fa-f:.]+\z/
    && $port =~ /\A[0-9]{1,5}\z/
    && $port > 0
    && $port < 65536;

my $sock = $class->new(
    PeerAddr => $host,
    PeerPort => $port,
    Proto    => 'tcp',
    Timeout  => 5,
) or do {
    print STDERR "connect to $host:$port failed: $!\n";
    exit 2;
};

binmode(STDIN);
binmode(STDOUT);
binmode($sock);
STDIN->blocking(0);
STDOUT->blocking(0);
$sock->blocking(0);

my ($up, $down) = ('', '');    # pending bytes: stdin -> socket, socket -> stdout
my ($stdin_eof, $sock_eof, $shut) = (0, 0, 0);
my $limit = 1 << 18;

sub would_block { return $!{EAGAIN} || $!{EWOULDBLOCK} || $!{EINTR} }

while (1) {
    my $rs = IO::Select->new;
    my $ws = IO::Select->new;
    $rs->add(\*STDIN) unless $stdin_eof || length($up) >= $limit;
    $rs->add($sock)   unless $sock_eof  || length($down) >= $limit;
    $ws->add($sock)    if length($up);
    $ws->add(\*STDOUT) if length($down);

    my ($r, $w) = IO::Select->select($rs, $ws, undef, undef);
    next unless $r && $w;    # undef on EINTR

    for my $fh (@$r) {
        my $n = sysread($fh, my $buf, 65536);
        if (!defined $n) {
            next if would_block();
            print STDERR "read failed: $!\n";
            exit 1;
        }
        if (fileno($fh) == 0) {
            if ($n == 0) { $stdin_eof = 1 } else { $up .= $buf }
        } else {
            if ($n == 0) { $sock_eof = 1 } else { $down .= $buf }
        }
    }

    for my $fh (@$w) {
        my $is_out = fileno($fh) == 1;
        my $n = syswrite($fh, $is_out ? $down : $up);
        if (!defined $n) {
            next if would_block();
            print STDERR "write failed: $!\n";
            exit 1;
        }
        substr($is_out ? $down : $up, 0, $n, '');
    }

    # Client is done sending: half-close, keep draining the reply.
    if ($stdin_eof && !$shut && !length($up)) {
        shutdown($sock, 1);
        $shut = 1;
    }
    # Server is done and everything it sent was delivered.
    exit 0 if $sock_eof && !length($down);
}
