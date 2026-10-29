# Usage: perl block_sounds.pl <decompiled net/minecraft dir> <out dir>
# Writes block_sounds.tsv (block -> sound type) and sound_types.tsv (type -> volume, pitch, break, step, place, hit, fall events).
use strict; use warnings;
my ($root, $out) = @ARGV;
sub slurp { local $/; open my $f, '<', $_[0] or die "$_[0]: $!"; <$f> }

my %event;   # SoundEvents constant -> event name
my $se = slurp("$root/sounds/SoundEvents.java");
while ($se =~ /public static final (?:SoundEvent|Holder\.Reference<SoundEvent>) (\w+) = register\w*\(\s*"([^"]+)"/g) { $event{$1} = $2 }

open my $types, '>', "$out/sound_types.tsv" or die;
print $types "# type\tvolume\tpitch\tbreak\tstep\tplace\thit\tfall — from 26.3 SoundType.java\n";
my $st = slurp("$root/world/level/block/SoundType.java");
while ($st =~ /public static final SoundType (\w+) = new SoundType\(\s*([-\d.]+)F,\s*([-\d.]+)F,(.*?)\);/sg) {
  my ($name, $vol, $pitch, $rest) = ($1, $2, $3, $4);
  my @ev = map { $event{$_} // '' } ($rest =~ /SoundEvents\.(\w+)/g);
  next unless @ev == 5;
  print $types join("\t", lc $name, $vol, $pitch, @ev), "\n";
}

my $src = slurp("$root/world/level/block/Blocks.java");
my (%helper, %block, %copy, @order);
while ($src =~ /static BlockBehaviour\.Properties (\w+)\((.*?)\n   \}/sg) { my ($n, $b) = ($1, $2); $helper{$n} = $1 if $b =~ /\.sound\(SoundType\.(\w+)\)/ }
my @decls = split /\n   public static final Block /, $src; shift @decls;
for my $d (@decls) {
  my ($name) = $d =~ /^(\w+) = /; next unless $name;
  ($d) = split /\n   (?:public|private|static)/, $d;
  my $s = $d =~ /\.sound\(SoundType\.(\w+)\)/ ? $1 : undef;
  if (!defined $s) { for my $h (keys %helper) { if ($d =~ /\b\Q$h\E\(/) { $s = $helper{$h}; last } } }
  $s //= $1 if $d =~ /SoundType\.(\w+)/;  # passed to a helper, e.g. logProperties(..., SoundType.WOOD)
  if (!defined $s && $d =~ /of(?:Full|Legacy)Copy\((\w+)\)/) { $copy{$name} = $1 }
  $block{$name} = $s; push @order, $name;
}
open my $blocks, '>', "$out/block_sounds.tsv" or die;
print $blocks "# block\tsound type — from 26.3 Blocks.java (default stone)\n";
for my $n (@order) {
  my $s = $block{$n}; my $src = $n; my $g = 0;
  while (!defined $s && exists $copy{$src} && $g++ < 8) { $src = $copy{$src}; $s = $block{$src} }
  print $blocks lc($n), "\t", lc($s // 'stone'), "\n";
}
