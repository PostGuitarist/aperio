"""Export NAFNet-SIDD width 32 to ONNX for Aperio's denoise plugin.

    python tools/export-nafnet.py NAFNet-SIDD-width32.pth nafnet_sidd_width32.onnx

NAFNet is by Liangyu Chen, Xiaojie Chu, Xiangyu Zhang and Jian Sun ("Simple
Baselines for Image Restoration", ECCV 2022), MIT licensed:
https://github.com/megvii-research/NAFNet. The network is written out here
from the paper and the authors' configuration for this checkpoint (width 32,
encoder blocks 2/2/4/8, 12 middle blocks, decoder blocks 2/2/2/2), so the
export doesn't run any downloaded code: the checkpoint is read as plain
tensors and must match these layers exactly.

The model takes and returns NCHW RGB in 0..1 (sRGB-encoded), with height and
width multiples of 16. With the versions pinned in .github/workflows/models.yml
the output is byte-for-byte reproducible; the workflow checks its SHA-256
against the one Aperio downloads.
"""
import sys, torch
from torch import nn
import torch.nn.functional as F

class LayerNorm2d(nn.Module):
    def __init__(self, c, eps=1e-6):
        super().__init__()
        self.weight = nn.Parameter(torch.ones(c)); self.bias = nn.Parameter(torch.zeros(c)); self.eps = eps
    def forward(self, x):
        mu = x.mean(1, keepdim=True)
        var = (x - mu).pow(2).mean(1, keepdim=True)
        return self.weight.view(1, -1, 1, 1) * (x - mu) / (var + self.eps).sqrt() + self.bias.view(1, -1, 1, 1)

def gate(x):
    a, b = x.chunk(2, dim=1)
    return a * b

class Block(nn.Module):
    def __init__(self, c):
        super().__init__()
        d = 2 * c
        self.conv1 = nn.Conv2d(c, d, 1); self.conv2 = nn.Conv2d(d, d, 3, padding=1, groups=d); self.conv3 = nn.Conv2d(d // 2, c, 1)
        self.sca = nn.Sequential(nn.AdaptiveAvgPool2d(1), nn.Conv2d(d // 2, d // 2, 1))
        self.conv4 = nn.Conv2d(c, 2 * c, 1); self.conv5 = nn.Conv2d(c, c, 1)
        self.norm1 = LayerNorm2d(c); self.norm2 = LayerNorm2d(c)
        self.beta = nn.Parameter(torch.zeros(1, c, 1, 1)); self.gamma = nn.Parameter(torch.zeros(1, c, 1, 1))
    def forward(self, inp):
        x = gate(self.conv2(self.conv1(self.norm1(inp))))
        x = self.conv3(x * self.sca(x))
        y = inp + x * self.beta
        return y + self.conv5(gate(self.conv4(self.norm2(y)))) * self.gamma

class NAFNet(nn.Module):
    def __init__(self, width=32, enc=(2, 2, 4, 8), mid=12, dec=(2, 2, 2, 2)):
        super().__init__()
        self.intro = nn.Conv2d(3, width, 3, padding=1); self.ending = nn.Conv2d(width, 3, 3, padding=1)
        self.encoders, self.decoders, self.ups, self.downs = nn.ModuleList(), nn.ModuleList(), nn.ModuleList(), nn.ModuleList()
        c = width
        for n in enc:
            self.encoders.append(nn.Sequential(*[Block(c) for _ in range(n)]))
            self.downs.append(nn.Conv2d(c, 2 * c, 2, 2)); c *= 2
        self.middle_blks = nn.Sequential(*[Block(c) for _ in range(mid)])
        for n in dec:
            self.ups.append(nn.Sequential(nn.Conv2d(c, 2 * c, 1, bias=False), nn.PixelShuffle(2))); c //= 2
            self.decoders.append(nn.Sequential(*[Block(c) for _ in range(n)]))
    def forward(self, inp):
        # The caller pads to a multiple of 16 (the network halves the size four times).
        x = self.intro(inp); skips = []
        for e, d in zip(self.encoders, self.downs):
            x = e(x); skips.append(x); x = d(x)
        x = self.middle_blks(x)
        for dcd, up, s in zip(self.decoders, self.ups, skips[::-1]):
            x = dcd(up(x) + s)
        return self.ending(x) + inp

pth, out = sys.argv[1], sys.argv[2]
net = NAFNet()
sd = torch.load(pth, map_location='cpu', weights_only=True)
net.load_state_dict(sd.get('params', sd), strict=True)
net.eval()
torch.onnx.export(net, torch.rand(1, 3, 256, 256), out, input_names=['input'], output_names=['output'], opset_version=17,
    dynamic_axes={'input': {0: 'n', 2: 'h', 3: 'w'}, 'output': {0: 'n', 2: 'h', 3: 'w'}}, dynamo=False)
print('exported', out)
