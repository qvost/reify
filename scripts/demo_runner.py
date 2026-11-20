#!/usr/bin/env python3
"""
Shaligram Labs — Reify Terminal Demo Simulator
Records pixel-perfect, cinematic demo output for Hacker News and Twitter.
"""

import sys
import time
import os

# ANSI Colors
RESET = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"
ITALIC = "\033[3m"

# Grayscale / Muted (Shaligram Warm Bone / Carbon)
WHITE = "\033[38;2;240;238;232m"
MUTED = "\033[38;2;140;138;132m"
DARK = "\033[38;2;60;60;58m"

# Accents
EMERALD = "\033[38;2;52;211;153m"
CYAN = "\033[38;2;56;189;248m"
AMBER = "\033[38;2;251;191;36m"
RED = "\033[38;2;248;113;113m"

def type_cmd(text, delay=0.04):
    sys.stdout.write(f"{BOLD}{WHITE}❯ {RESET}")
    sys.stdout.flush()
    time.sleep(0.5)
    for char in text:
        sys.stdout.write(f"{BOLD}{WHITE}{char}{RESET}")
        sys.stdout.flush()
        time.sleep(delay)
    time.sleep(0.3)
    sys.stdout.write("\n")
    sys.stdout.flush()

def print_banner():
    print(f"\n{BOLD}{WHITE}  SHALIGRAM LABS {MUTED}• {WHITE}REIFY v0.1.0{RESET}")
    print(f"{MUTED}  Ephemeral Wire-Level MCP Compiler for Web Agents{RESET}\n")

def run_stage(num, title, detail, delay=0.6):
    sys.stdout.write(f"  {MUTED}[0{num}/04]{RESET} {BOLD}{WHITE}{title:<34}{RESET} ")
    sys.stdout.flush()
    time.sleep(delay * 0.6)
    sys.stdout.write(f"{EMERALD}✓{RESET} {MUTED}{detail}{RESET}\n")
    sys.stdout.flush()
    time.sleep(delay * 0.4)

def main():
    os.system("clear")
    time.sleep(0.8)

    # 1. User types command
    type_cmd("reify https://news.ycombinator.com \"search stories\" --port 8080", delay=0.035)
    time.sleep(0.2)

    # 2. Banner
    print_banner()
    time.sleep(0.4)

    # 3. 4 Pipeline Stages
    run_stage(1, "Wire Interception", "Zero-browser SSR sniffer (49 visual assets dropped)", 0.5)
    run_stage(2, "Dynamic Route Clustering", "1,420 frames collapsed -> 4 parameterized endpoints", 0.5)
    run_stage(3, "Schema Induction", "Draft 2020-12 types inferred in 14.8 ms (zero LLM)", 0.5)
    run_stage(4, "Stateless MCP Mount", "Axum streamable HTTP listening on 127.0.0.1:8080/mcp", 0.6)

    print()
    time.sleep(0.6)

    # 4. Agent Tool Call Simulation
    sys.stdout.write(f"  {AMBER}●{RESET} {BOLD}{WHITE}Agent Request:{RESET} {MUTED}POST /mcp HTTP/1.1{RESET}\n")
    sys.stdout.write(f"    {MUTED}Method:{RESET} {CYAN}tools/call{RESET} {MUTED}(name=\"hn_search_stories\", q=\"LLM latency\"){RESET}\n")
    sys.stdout.flush()
    time.sleep(0.5)

    sys.stdout.write(f"  {EMERALD}●{RESET} {BOLD}{WHITE}Tool Dispatched:{RESET} {EMERALD}200 OK{RESET} {MUTED}in{RESET} {BOLD}{EMERALD}4.1 µs{RESET} {MUTED}(30 stories streamed){RESET}\n")
    sys.stdout.flush()
    print()
    time.sleep(0.8)

    # 5. Benchmark Comparison Table
    print(f"  {BOLD}{WHITE}┌─────────────────────────┬────────────────────┬────────────────────┐{RESET}")
    print(f"  {BOLD}{WHITE}│ Metric                  │ Visual Browser-Use │ Reify Wire Synth   │{RESET}")
    print(f"  {BOLD}{WHITE}├─────────────────────────┼────────────────────┼────────────────────┤{RESET}")
    print(f"  {BOLD}{WHITE}│ Context Tokens          │{RESET} {RED}24,100 tokens      {RESET}{BOLD}{WHITE}│{RESET} {EMERALD}380 tokens (-98.4%) {RESET}{BOLD}{WHITE}│{RESET}")
    print(f"  {BOLD}{WHITE}│ Cold Compile Latency    │{RESET} {MUTED}—                  {RESET}{BOLD}{WHITE}│{RESET} {EMERALD}14.8 ms (SSR fast)  {RESET}{BOLD}{WHITE}│{RESET}")
    print(f"  {BOLD}{WHITE}│ Request Dispatch Time   │{RESET} {RED}38,400,000 µs (38s) {RESET}{BOLD}{WHITE}│{RESET} {EMERALD}4.1 µs (>240k req/s){RESET}{BOLD}{WHITE}│{RESET}")
    print(f"  {BOLD}{WHITE}│ Cost Per Action Step    │{RESET} {RED}$0.048 / step      {RESET}{BOLD}{WHITE}│{RESET} {EMERALD}$0.000000 / step    {RESET}{BOLD}{WHITE}│{RESET}")
    print(f"  {BOLD}{WHITE}└─────────────────────────┴────────────────────┴────────────────────┘{RESET}")
    print()
    time.sleep(1.0)

    # 6. Outro
    print(f"  {BOLD}{WHITE}Documentation & Research Paper:{RESET} {CYAN}https://shaligramlabs.vercel.app{RESET}")
    print(f"  {BOLD}{WHITE}Open Source Engine:{RESET}             {CYAN}https://github.com/qvost/reify{RESET}\n")
    time.sleep(4.0)

if __name__ == "__main__":
    main()
