# Lists the blocks you walk through (`.noCollision()` in Blocks.java), following helper methods and property copies.
use strict; use warnings;
local $/; my $src = <STDIN>;
my (%helper, %none, %copy, @order);
while ($src =~ /static BlockBehaviour\.Properties (\w+)\((.*?)\n   \}/sg) {
  my ($name, $body) = ($1, $2);
  $helper{$name} = 1 if $body =~ /\.noCollision\(\)/;
}
my @decls = split /\n   public static final Block /, $src;
shift @decls;
for my $d (@decls) {
  my ($name) = $d =~ /^(\w+) = /; next unless $name;
  ($d) = split /\n   (?:public|private|static)/, $d;
  my $n = $d =~ /\.noCollision\(\)/ ? 1 : undef;
  if (!defined $n) {
    for my $hn (keys %helper) { if ($d =~ /\b\Q$hn\E\(/) { $n = 1; last } }
  }
  if (!defined $n && $d =~ /(?:of(?:Full|Legacy)Copy|wallVariant)\((\w+)/) { $copy{$name} = $1 }
  $none{$name} = $n; push @order, $name;
}
for my $b (@order) {
  my $n = $none{$b}; my $s = $b; my $guard = 0;
  while (!defined $n && exists $copy{$s} && $guard++ < 8) { $s = $copy{$s}; $n = $none{$s} }
  print lc($b), "\n" if $n;
}
