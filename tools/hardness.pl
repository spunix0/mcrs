# Extracts (block, hardness, requires_correct_tool) from a decompiled Blocks.java.
use strict; use warnings;
local $/; my $src = <STDIN>;
my (%helper, %block, %copy, %calls, @order);
# Helper methods returning Properties, e.g. logProperties(...)
while ($src =~ /static BlockBehaviour\.Properties (\w+)\((.*?)\n   \}/sg) {
  my ($n, $body) = ($1, $2);
  $helper{$n} = [ ($body =~ /\.strength\(([-\d.]+)F/ ? $1 : ($body =~ /instabreak/ ? 0 : undef)), ($body =~ /requiresCorrectToolForDrops/ ? 1 : 0) ];
}
my @decls = split /\n   public static final Block /, $src;
shift @decls;
for my $d (@decls) {
  my ($name) = $d =~ /^(\w+) = /; next unless $name;
  ($d) = split /\n   (?:public|private|static)/, $d;
  my $h = $d =~ /\.strength\(([-\d.]+)F/ ? $1 : ($d =~ /\.instabreak\(\)/ ? 0 : undef);
  my $req = $d =~ /requiresCorrectToolForDrops/ ? 1 : 0;
  if (!defined $h) {
    for my $hn (keys %helper) { if ($d =~ /\b\Q$hn\E\(/ && defined $helper{$hn}[0]) { $h = $helper{$hn}[0]; $req ||= $helper{$hn}[1]; last } }
  }
  if (!defined $h && $d =~ /of(?:Full|Legacy)Copy\((\w+)\)/) { $copy{$name} = $1 }
  $block{$name} = [ $h, $req ]; push @order, $name;
}
for my $n (@order) {
  my ($h, $req) = @{$block{$n}};
  my $src = $n; my $guard = 0;
  while (!defined $h && exists $copy{$src} && $guard++ < 8) { $src = $copy{$src}; ($h, my $r2) = @{$block{$src} // [undef, 0]}; $req ||= $r2 }
  $h //= 0;
  printf "%s\t%s\t%d\n", lc $n, $h, $req;
}
