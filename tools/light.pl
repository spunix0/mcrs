# Extracts (block, light emission rule) from a decompiled Blocks.java, for blocks that give off light.
# Rules: a number, lit:N (N while lit=true), candle, level, berries:N, pickle, anchor. Copper bulbs (in a
# WeatheringCopperCollection) and the trial spawner/vault (state-dependent) are handled in mc-assets by hand.
use strict; use warnings;
local $/; my $src = <STDIN>;
sub rule {
  my ($d) = @_;
  return $1 if $d =~ /lightLevel\(state -> (\d+)\)/;
  return "lit:$1" if $d =~ /lightLevel\(litBlockEmission\((\d+)\)\)/;
  return "candle" if $d =~ /CandleBlock\.LIGHT_EMISSION/;
  return "level" if $d =~ /LightBlock\.LIGHT_EMISSION/;
  return $1 if $d =~ /GlowLichenBlock\.emission\((\d+)\)/;
  return "berries:$1" if $d =~ /CaveVines\.emission\((\d+)\)/;
  return "pickle" if $d =~ /lightLevel\(state -> SeaPickleBlock/;
  return "anchor" if $d =~ /getScaledChargeLevel/;
  return undef;
}
my (%helper, %rule, %copy, @order);
while ($src =~ /static BlockBehaviour\.Properties (\w+)\((.*?)\n   \}/sg) {
  my $r = rule($2); $helper{$1} = $r if defined $r;
}
my @decls = split /\n   public static final Block /, $src;
shift @decls;
for my $d (@decls) {
  my ($name) = $d =~ /^(\w+) = /; next unless $name;
  ($d) = split /\n   (?:public|private|static)/, $d;
  my $r = rule($d);
  if (!defined $r) {
    for my $hn (keys %helper) { if ($d =~ /\b\Q$hn\E\(/) { $r = $helper{$hn}; last } }
  }
  if (!defined $r && $d =~ /(?:of(?:Full|Legacy)Copy|wallVariant)\((\w+)/) { $copy{$name} = $1 }
  $rule{$name} = $r; push @order, $name;
}
for my $n (@order) {
  my $r = $rule{$n}; my $s = $n; my $guard = 0;
  while (!defined $r && exists $copy{$s} && $guard++ < 8) { $s = $copy{$s}; $r = $rule{$s} }
  printf "%s\t%s\n", lc $n, $r if defined $r && $r ne "0";
}
