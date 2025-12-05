#!/bin/bash
# MANA Stream-JSON Parser
# Converts Claude Code stream-json output to human-readable format
# Version 2.0 - Full content display, no truncation

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
WHITE='\033[1;37m'
GRAY='\033[0;90m'
NC='\033[0m' # No Color
BOLD='\033[1m'
DIM='\033[2m'

# Track state
CURRENT_TOOL=""
IN_THINKING=false

# Process each line
while IFS= read -r line; do
    # Skip empty lines
    [ -z "$line" ] && continue

    # Try to parse as JSON
    if ! echo "$line" | jq -e . >/dev/null 2>&1; then
        # Not JSON, print as-is
        echo "$line"
        continue
    fi

    # Extract type
    TYPE=$(echo "$line" | jq -r '.type // empty' 2>/dev/null)

    case "$TYPE" in
        "system")
            # System init message - show abbreviated info
            SUBTYPE=$(echo "$line" | jq -r '.subtype // empty' 2>/dev/null)
            if [ "$SUBTYPE" = "init" ]; then
                SESSION=$(echo "$line" | jq -r '.session_id // empty' 2>/dev/null)
                MODEL=$(echo "$line" | jq -r '.model // empty' 2>/dev/null)
                VERSION=$(echo "$line" | jq -r '.claude_code_version // empty' 2>/dev/null)
                TOOL_COUNT=$(echo "$line" | jq -r '.tools | length // 0' 2>/dev/null)
                MCP_SERVERS=$(echo "$line" | jq -r '.mcp_servers | map(.name) | join(", ") // empty' 2>/dev/null)
                echo -e "${BLUE}${BOLD}╔══ Session Init ══╗${NC}"
                echo -e "${BLUE}│ Session:${NC} ${SESSION:0:8}..."
                echo -e "${BLUE}│ Model:${NC} $MODEL"
                echo -e "${BLUE}│ Version:${NC} $VERSION"
                echo -e "${BLUE}│ Tools:${NC} $TOOL_COUNT available"
                echo -e "${BLUE}│ MCP:${NC} $MCP_SERVERS"
                echo -e "${BLUE}╚═══════════════════╝${NC}"
            else
                MSG=$(echo "$line" | jq -r '.message // empty' 2>/dev/null)
                if [ -n "$MSG" ]; then
                    echo -e "${GRAY}[system] $MSG${NC}"
                fi
            fi
            ;;

        "assistant")
            # Assistant messages - can contain text or tool_use in content array
            CONTENT_TYPE=$(echo "$line" | jq -r '.message.content[0].type // empty' 2>/dev/null)

            case "$CONTENT_TYPE" in
                "text")
                    TEXT=$(echo "$line" | jq -r '.message.content[0].text // empty' 2>/dev/null)
                    if [ -n "$TEXT" ] && [ "$TEXT" != "null" ]; then
                        echo ""
                        echo -e "${WHITE}${BOLD}Claude:${NC}"
                        echo "$TEXT" | while IFS= read -r text_line; do
                            echo -e "${WHITE}  $text_line${NC}"
                        done
                    fi
                    ;;
                "tool_use")
                    # Tool call from assistant
                    TOOL_NAME=$(echo "$line" | jq -r '.message.content[0].name // empty' 2>/dev/null)
                    TOOL_ID=$(echo "$line" | jq -r '.message.content[0].id // empty' 2>/dev/null)
                    TOOL_INPUT=$(echo "$line" | jq -r '.message.content[0].input // {}' 2>/dev/null)

                    echo ""
                    echo -e "${CYAN}${BOLD}▶ Tool: $TOOL_NAME${NC}"

                    # Show full input, nicely formatted
                    if [ -n "$TOOL_INPUT" ] && [ "$TOOL_INPUT" != "{}" ]; then
                        echo -e "${GRAY}┌─ Input ─────────────────────────────────────────${NC}"
                        echo "$TOOL_INPUT" | jq -r '.' 2>/dev/null | while IFS= read -r input_line; do
                            echo -e "${GRAY}│ $input_line${NC}"
                        done
                        echo -e "${GRAY}└─────────────────────────────────────────────────${NC}"
                    fi
                    ;;
            esac
            ;;

        "user")
            # User messages - usually tool results
            TOOL_RESULT=$(echo "$line" | jq -r '.message.content[0].type // empty' 2>/dev/null)
            if [ "$TOOL_RESULT" = "tool_result" ]; then
                TOOL_ID=$(echo "$line" | jq -r '.message.content[0].tool_use_id // empty' 2>/dev/null)
                IS_ERROR=$(echo "$line" | jq -r '.message.content[0].is_error // false' 2>/dev/null)
                CONTENT=$(echo "$line" | jq -r '.message.content[0].content // empty' 2>/dev/null)

                # Also check tool_use_result for stdout
                STDOUT=$(echo "$line" | jq -r '.tool_use_result.stdout // empty' 2>/dev/null)
                STDERR=$(echo "$line" | jq -r '.tool_use_result.stderr // empty' 2>/dev/null)

                if [ "$IS_ERROR" = "true" ]; then
                    echo -e "${RED}${BOLD}✗ Error:${NC}"
                    echo -e "${RED}┌─ Error Output ──────────────────────────────────${NC}"
                    echo "$CONTENT" | while IFS= read -r result_line; do
                        echo -e "${RED}│ $result_line${NC}"
                    done
                    echo -e "${RED}└─────────────────────────────────────────────────${NC}"
                else
                    echo -e "${GREEN}${BOLD}✓ Result:${NC}"

                    # Use STDOUT if available, otherwise use CONTENT
                    OUTPUT="${STDOUT:-$CONTENT}"

                    if [ -n "$OUTPUT" ]; then
                        LINE_COUNT=$(echo "$OUTPUT" | wc -l)
                        echo -e "${GRAY}┌─ Output ($LINE_COUNT lines) ───────────────────────────${NC}"
                        echo "$OUTPUT" | while IFS= read -r result_line; do
                            echo -e "${GRAY}│ $result_line${NC}"
                        done
                        echo -e "${GRAY}└─────────────────────────────────────────────────${NC}"
                    fi

                    # Show stderr if present
                    if [ -n "$STDERR" ]; then
                        echo -e "${YELLOW}┌─ Stderr ────────────────────────────────────────${NC}"
                        echo "$STDERR" | while IFS= read -r err_line; do
                            echo -e "${YELLOW}│ $err_line${NC}"
                        done
                        echo -e "${YELLOW}└─────────────────────────────────────────────────${NC}"
                    fi
                fi
            fi
            ;;

        "content_block_start")
            BLOCK_TYPE=$(echo "$line" | jq -r '.content_block.type // empty' 2>/dev/null)
            case "$BLOCK_TYPE" in
                "tool_use")
                    TOOL_NAME=$(echo "$line" | jq -r '.content_block.name // empty' 2>/dev/null)
                    CURRENT_TOOL="$TOOL_NAME"
                    echo -e "\n${CYAN}${BOLD}▶ Tool: $TOOL_NAME${NC}"
                    ;;
                "thinking")
                    IN_THINKING=true
                    echo -e "\n${MAGENTA}${DIM}💭 Thinking...${NC}"
                    ;;
                "text")
                    ;;
            esac
            ;;

        "content_block_delta")
            DELTA_TYPE=$(echo "$line" | jq -r '.delta.type // empty' 2>/dev/null)
            case "$DELTA_TYPE" in
                "text_delta")
                    TEXT=$(echo "$line" | jq -r '.delta.text // empty' 2>/dev/null)
                    if [ -n "$TEXT" ]; then
                        if [ "$IN_THINKING" = true ]; then
                            echo -ne "${MAGENTA}${DIM}$TEXT${NC}"
                        else
                            echo -ne "${WHITE}$TEXT${NC}"
                        fi
                    fi
                    ;;
                "input_json_delta")
                    # Tool input being streamed - show ALL of it
                    JSON=$(echo "$line" | jq -r '.delta.partial_json // empty' 2>/dev/null)
                    if [ -n "$JSON" ]; then
                        echo -ne "${GRAY}$JSON${NC}"
                    fi
                    ;;
            esac
            ;;

        "content_block_stop")
            if [ "$IN_THINKING" = true ]; then
                IN_THINKING=false
                echo -e "\n${MAGENTA}${DIM}💭 Done thinking${NC}"
            fi
            echo ""
            ;;

        "tool_use")
            # Direct tool_use event
            TOOL=$(echo "$line" | jq -r '.name // empty' 2>/dev/null)
            echo -e "\n${CYAN}${BOLD}▶ Tool: $TOOL${NC}"

            # Show FULL input
            INPUT=$(echo "$line" | jq -r '.input // {}' 2>/dev/null)
            if [ -n "$INPUT" ] && [ "$INPUT" != "{}" ]; then
                echo -e "${GRAY}┌─ Input ─────────────────────────────────────────${NC}"
                echo "$INPUT" | jq -r '.' 2>/dev/null | while IFS= read -r input_line; do
                    echo -e "${GRAY}│ $input_line${NC}"
                done
                echo -e "${GRAY}└─────────────────────────────────────────────────${NC}"
            fi
            ;;

        "tool_result")
            # Direct tool_result event
            TOOL_ID=$(echo "$line" | jq -r '.tool_use_id // empty' 2>/dev/null)
            IS_ERROR=$(echo "$line" | jq -r '.is_error // false' 2>/dev/null)
            CONTENT=$(echo "$line" | jq -r '.content // empty' 2>/dev/null)

            if [ "$IS_ERROR" = "true" ]; then
                echo -e "${RED}${BOLD}✗ Error:${NC}"
                echo -e "${RED}┌─ Error Output ──────────────────────────────────${NC}"
                echo "$CONTENT" | while IFS= read -r result_line; do
                    echo -e "${RED}│ $result_line${NC}"
                done
                echo -e "${RED}└─────────────────────────────────────────────────${NC}"
            else
                LINE_COUNT=$(echo "$CONTENT" | wc -l)
                echo -e "${GREEN}${BOLD}✓ Result:${NC}"
                echo -e "${GRAY}┌─ Output ($LINE_COUNT lines) ───────────────────────────${NC}"
                echo "$CONTENT" | while IFS= read -r result_line; do
                    echo -e "${GRAY}│ $result_line${NC}"
                done
                echo -e "${GRAY}└─────────────────────────────────────────────────${NC}"
            fi
            ;;

        "result")
            # Final result
            echo -e "\n${GREEN}${BOLD}═══════════════════════════════════════════════════${NC}"
            echo -e "${GREEN}${BOLD}                    RESULT                         ${NC}"
            echo -e "${GREEN}${BOLD}═══════════════════════════════════════════════════${NC}"

            RESULT=$(echo "$line" | jq -r '.result // empty' 2>/dev/null)
            if [ -n "$RESULT" ]; then
                echo "$RESULT" | while IFS= read -r result_line; do
                    echo -e "${WHITE}$result_line${NC}"
                done
            fi

            # Show cost if available
            COST=$(echo "$line" | jq -r '.cost_usd // empty' 2>/dev/null)
            INPUT_TOKENS=$(echo "$line" | jq -r '.usage.input_tokens // empty' 2>/dev/null)
            OUTPUT_TOKENS=$(echo "$line" | jq -r '.usage.output_tokens // empty' 2>/dev/null)

            echo ""
            echo -e "${GRAY}───────────────────────────────────────────────────${NC}"
            if [ -n "$COST" ] && [ "$COST" != "null" ]; then
                echo -e "${GRAY}💰 Cost: \$$COST${NC}"
            fi
            if [ -n "$INPUT_TOKENS" ] && [ "$INPUT_TOKENS" != "null" ]; then
                echo -e "${GRAY}📥 Input tokens: $INPUT_TOKENS${NC}"
            fi
            if [ -n "$OUTPUT_TOKENS" ] && [ "$OUTPUT_TOKENS" != "null" ]; then
                echo -e "${GRAY}📤 Output tokens: $OUTPUT_TOKENS${NC}"
            fi

            # Show duration if available
            DURATION=$(echo "$line" | jq -r '.duration_ms // empty' 2>/dev/null)
            if [ -n "$DURATION" ] && [ "$DURATION" != "null" ]; then
                echo -e "${GRAY}⏱️  Duration: ${DURATION}ms${NC}"
            fi
            echo -e "${GRAY}───────────────────────────────────────────────────${NC}"
            ;;

        "error")
            # Error message
            ERROR=$(echo "$line" | jq -r '.error.message // .message // empty' 2>/dev/null)
            echo -e "\n${RED}${BOLD}╔══ ERROR ══════════════════════════════════════════╗${NC}"
            echo -e "${RED}${BOLD}║ $ERROR${NC}"
            echo -e "${RED}${BOLD}╚═══════════════════════════════════════════════════╝${NC}"
            ;;

        "message_start"|"message_delta"|"message_stop")
            # Message lifecycle events - ignore silently
            ;;

        *)
            # Handle custom events (iteration markers, etc)
            EVENT=$(echo "$line" | jq -r '.event // empty' 2>/dev/null)
            if [ -n "$EVENT" ]; then
                case "$EVENT" in
                    "iteration_start")
                        ITER=$(echo "$line" | jq -r '.iteration // empty' 2>/dev/null)
                        TS=$(echo "$line" | jq -r '.timestamp // empty' 2>/dev/null)
                        echo -e "${YELLOW}${BOLD}🔄 Iteration $ITER started at $TS${NC}"
                        ;;
                    "iteration_end")
                        ITER=$(echo "$line" | jq -r '.iteration // empty' 2>/dev/null)
                        DUR=$(echo "$line" | jq -r '.duration_secs // empty' 2>/dev/null)
                        echo -e "${YELLOW}${BOLD}✅ Iteration $ITER ended (${DUR}s)${NC}"
                        ;;
                    *)
                        echo -e "${GRAY}[event:$EVENT] $(echo "$line" | jq -c '.')${NC}"
                        ;;
                esac
            elif [ ${#line} -lt 500 ]; then
                # Unknown type - show raw if reasonably small
                echo -e "${GRAY}[${TYPE:-unknown}] $(echo "$line" | jq -c '.' 2>/dev/null || echo "$line")${NC}"
            fi
            ;;
    esac
done
